use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use phi_contracts::SLOT_SANDBOX_EXECUTE;
use phi_contracts::ports::{ExecOutput, SandboxPort};
use phi_i18n as i18n;
use phi_kernel::lifecycle::{Plugin, PluginContext};
use phi_kernel::registry::Contribution;

pub struct FsSandboxPlugin;

impl Plugin for FsSandboxPlugin {
    fn id(&self) -> &'static str {
        "plugin-sandbox"
    }

    fn activate(&self, _ctx: &PluginContext) -> Result<Vec<Contribution>, String> {
        Ok(vec![Contribution {
            plugin_id: self.id().to_string(),
            slot: SLOT_SANDBOX_EXECUTE.to_string(),
            id: "fs-jail".to_string(),
        }])
    }
}

#[async_trait]
impl SandboxPort for FsSandboxPlugin {
    fn resolve_path(&self, workspace: &Path, requested: &str) -> Result<PathBuf, String> {
        let workspace = workspace.canonicalize().map_err(|e| {
            i18n::t_args(
                "Workspace is not available: {error}",
                &[("error", e.to_string())],
            )
        })?;
        let requested = requested.trim();
        let requested = if requested.is_empty() { "." } else { requested };
        let relative = Path::new(requested);

        for component in relative.components() {
            match component {
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(i18n::t("Access outside the workspace is denied"));
                }
                _ => {}
            }
        }

        let joined = workspace.join(relative);
        let mut ancestor = joined.as_path();
        while !ancestor.exists() {
            ancestor = ancestor.parent().ok_or_else(|| i18n::t("Invalid path"))?;
            if ancestor.as_os_str().is_empty() {
                return Err(i18n::t("Invalid path"));
            }
        }
        let canonical_ancestor = ancestor.canonicalize().map_err(|e| {
            i18n::t_args(
                "Path is not available: {error}",
                &[("error", e.to_string())],
            )
        })?;
        if !canonical_ancestor.starts_with(&workspace) {
            return Err(i18n::t("Access outside the workspace is denied"));
        }
        let suffix = joined
            .strip_prefix(ancestor)
            .map_err(|_| i18n::t("Failed to resolve path"))?;
        Ok(canonical_ancestor.join(suffix))
    }

    async fn exec(
        &self,
        workspace: &Path,
        command: &str,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> Result<ExecOutput, String> {
        let mut cmd = build_command(command);
        cmd.current_dir(workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd.spawn().map_err(|e| {
            i18n::t_args(
                "Failed to start command: {error}",
                &[("error", e.to_string())],
            )
        })?;
        let pid = child.id();

        let mut stdout_pipe = child.stdout.take();
        let mut stderr_pipe = child.stderr.take();
        let stdout_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(pipe) = stdout_pipe.as_mut() {
                let mut limited = pipe.take(1_000_000);
                let _ = limited.read_to_end(&mut buf).await;
            }
            buf
        });
        let stderr_task = tokio::spawn(async move {
            let mut buf = Vec::new();
            if let Some(pipe) = stderr_pipe.as_mut() {
                let mut limited = pipe.take(1_000_000);
                let _ = limited.read_to_end(&mut buf).await;
            }
            buf
        });

        let deadline = tokio::time::Instant::now() + timeout;
        let mut timed_out = false;
        let mut cancelled = false;
        let mut status = None;

        loop {
            if cancel.is_cancelled() {
                cancelled = true;
                kill_process_tree(pid, &mut child).await;
                break;
            }
            match child.try_wait() {
                Ok(Some(exit)) => {
                    status = Some(exit);
                    break;
                }
                Ok(None) => {}
                Err(e) => {
                    return Err(i18n::t_args(
                        "Failed to wait for command: {error}",
                        &[("error", e.to_string())],
                    ));
                }
            }
            if tokio::time::Instant::now() >= deadline {
                timed_out = true;
                kill_process_tree(pid, &mut child).await;
                break;
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }

        if status.is_none() {
            status = child.wait().await.ok();
        }

        let collect = |task: tokio::task::JoinHandle<Vec<u8>>| async move {
            match tokio::time::timeout(Duration::from_secs(2), task).await {
                Ok(Ok(buf)) => buf,
                _ => Vec::new(),
            }
        };
        let stdout = collect(stdout_task).await;
        let stderr = collect(stderr_task).await;

        if cancelled {
            return Err("cancelled".to_string());
        }

        Ok(ExecOutput {
            exit_code: status.and_then(|s| s.code()),
            stdout: String::from_utf8_lossy(&stdout).to_string(),
            stderr: String::from_utf8_lossy(&stderr).to_string(),
            timed_out,
        })
    }
}

fn build_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    }
}

async fn kill_process_tree(pid: Option<u32>, child: &mut tokio::process::Child) {
    #[cfg(windows)]
    if let Some(pid) = pid {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
    let _ = child.start_kill();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_ws(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("phi-ws-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rejects_parent_escape() {
        let ws = temp_ws("escape");
        let sandbox = FsSandboxPlugin;
        assert!(sandbox.resolve_path(&ws, "../secret.txt").is_err());
        assert!(sandbox.resolve_path(&ws, "a/../../secret.txt").is_err());
        assert!(sandbox.resolve_path(&ws, "C:/Windows/win.ini").is_err());
    }

    #[test]
    fn allows_workspace_relative() {
        let ws = temp_ws("inside");
        let sandbox = FsSandboxPlugin;
        let resolved = sandbox.resolve_path(&ws, "src/main.rs").unwrap();
        assert!(resolved.starts_with(ws.canonicalize().unwrap()));
    }

    #[tokio::test]
    async fn executes_command_in_workspace() {
        let ws = temp_ws("exec");
        let sandbox = FsSandboxPlugin;
        let out = sandbox
            .exec(
                &ws,
                "echo phi-sandbox-ok",
                Duration::from_secs(10),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(out.stdout.contains("phi-sandbox-ok"));
    }
}
