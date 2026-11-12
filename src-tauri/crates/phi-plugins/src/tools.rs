use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use phi_contracts::SLOT_TOOL_PROVIDER;
use phi_contracts::ports::{
    SandboxPort, SideEffect, ToolCall, ToolContext, ToolDefinition, ToolOutput, ToolPort,
};
use phi_contracts::util::truncate_chars;
use phi_i18n as i18n;
use phi_kernel::lifecycle::{Plugin, PluginContext};
use phi_kernel::registry::Contribution;

pub struct BuiltinToolsPlugin {
    sandbox: Arc<dyn SandboxPort>,
    backup_dir: PathBuf,
}

impl BuiltinToolsPlugin {
    pub fn new(sandbox: Arc<dyn SandboxPort>, backup_dir: PathBuf) -> Self {
        let _ = fs::create_dir_all(&backup_dir);
        Self {
            sandbox,
            backup_dir,
        }
    }
}

impl Plugin for BuiltinToolsPlugin {
    fn id(&self) -> &'static str {
        "plugin-tools"
    }

    fn activate(&self, _ctx: &PluginContext) -> Result<Vec<Contribution>, String> {
        Ok(vec![Contribution {
            plugin_id: self.id().to_string(),
            slot: SLOT_TOOL_PROVIDER.to_string(),
            id: "builtin".to_string(),
        }])
    }
}

fn tool(
    id: &str,
    name: &str,
    description: &str,
    properties: Value,
    required: Vec<&str>,
    side_effect: SideEffect,
) -> ToolDefinition {
    ToolDefinition {
        id: id.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        input_schema: json!({
            "type": "object",
            "properties": properties,
            "required": required,
        }),
        side_effect,
    }
}

#[async_trait]
impl ToolPort for BuiltinToolsPlugin {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![
            tool(
                "fs_list",
                "fs_list",
                "List files and directories inside the workspace. Path is relative to the workspace root.",
                json!({ "path": { "type": "string", "description": "Directory to list, default \".\"" } }),
                vec![],
                SideEffect::None,
            ),
            tool(
                "fs_read",
                "fs_read",
                "Read a UTF-8 text file inside the workspace. Returns numbered lines.",
                json!({
                    "path": { "type": "string", "description": "File path relative to the workspace root" },
                    "offset": { "type": "integer", "description": "1-based first line, default 1" },
                    "limit": { "type": "integer", "description": "Max lines to return, default 400" }
                }),
                vec!["path"],
                SideEffect::None,
            ),
            tool(
                "fs_write",
                "fs_write",
                "Create or overwrite a text file inside the workspace. Requires human approval. The previous content is backed up.",
                json!({
                    "path": { "type": "string", "description": "File path relative to the workspace root" },
                    "content": { "type": "string", "description": "Full new file content" },
                    "create_dirs": { "type": "boolean", "description": "Create parent directories, default false" }
                }),
                vec!["path", "content"],
                SideEffect::Reversible,
            ),
            tool(
                "shell_exec",
                "shell_exec",
                "Run a shell command with the workspace as cwd. Requires human approval. Returns stdout/stderr and exit code.",
                json!({
                    "command": { "type": "string", "description": "Command line to execute" },
                    "timeout_secs": { "type": "integer", "description": "Timeout in seconds, default 120, max 300" }
                }),
                vec!["command"],
                SideEffect::Irreversible,
            ),
        ]
    }

    async fn invoke(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, String> {
        if ctx
            .deadline
            .map(|deadline| std::time::Instant::now() >= deadline)
            .unwrap_or(false)
        {
            return Ok(ToolOutput {
                content: i18n::t("Wall-clock limit exceeded; action was not executed."),
                is_error: true,
            });
        }
        let result = match call.tool_id.as_str() {
            "fs_list" => self.fs_list(&call.input, &ctx.workspace),
            "fs_read" => self.fs_read(&call.input, &ctx.workspace),
            "fs_write" => self.fs_write(&call.input, &ctx.workspace),
            "shell_exec" => {
                let command = call
                    .input
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                if command.trim().is_empty() {
                    return Ok(ToolOutput {
                        content: i18n::t("Command is empty"),
                        is_error: true,
                    });
                }
                let mut timeout_secs = call
                    .input
                    .get("timeout_secs")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(120)
                    .clamp(1, 300);
                if let Some(deadline) = ctx.deadline {
                    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                    if remaining.is_zero() {
                        return Ok(ToolOutput {
                            content: i18n::t("Wall-clock limit exceeded; action was not executed."),
                            is_error: true,
                        });
                    }
                    timeout_secs = timeout_secs.min(remaining.as_secs().max(1));
                }
                return match self
                    .sandbox
                    .exec(
                        &ctx.workspace,
                        &command,
                        Duration::from_secs(timeout_secs),
                        cancel,
                    )
                    .await
                {
                    Ok(out) => {
                        let mut text = String::new();
                        if out.timed_out {
                            text.push_str(&i18n::t_args(
                                "(command timed out after {secs}s; terminated)\n",
                                &[("secs", timeout_secs.to_string())],
                            ));
                        }
                        text.push_str(&format!("exit code: {:?}\n", out.exit_code));
                        text.push_str("--- stdout ---\n");
                        text.push_str(out.stdout.trim_end());
                        text.push_str("\n--- stderr ---\n");
                        text.push_str(out.stderr.trim_end());
                        Ok(ToolOutput {
                            content: truncate_chars(&text, 20_000),
                            is_error: out.exit_code != Some(0),
                        })
                    }
                    Err(err) if err == "cancelled" => Err(err),
                    Err(err) => Ok(ToolOutput {
                        content: i18n::t_args(
                            "Command execution failed: {error}",
                            &[("error", err)],
                        ),
                        is_error: true,
                    }),
                };
            }
            other => {
                return Ok(ToolOutput {
                    content: i18n::t_args("Unknown tool: {tool}", &[("tool", other.to_string())]),
                    is_error: true,
                });
            }
        };

        Ok(match result {
            Ok(content) => ToolOutput {
                content: truncate_chars(&content, 20_000),
                is_error: false,
            },
            Err(message) => ToolOutput {
                content: message,
                is_error: true,
            },
        })
    }
}

impl BuiltinToolsPlugin {
    fn fs_list(&self, args: &Value, workspace: &Path) -> Result<String, String> {
        let requested = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let dir = self.sandbox.resolve_path(workspace, requested)?;
        if !dir.is_dir() {
            return Err(i18n::t_args(
                "Not a directory: {path}",
                &[("path", requested.to_string())],
            ));
        }
        let mut entries: Vec<String> = Vec::new();
        for entry in fs::read_dir(&dir).map_err(|e| {
            i18n::t_args(
                "Failed to read directory: {error}",
                &[("error", e.to_string())],
            )
        })? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name().to_string_lossy().to_string();
            let meta = entry.metadata().map_err(|e| e.to_string())?;
            if meta.is_dir() {
                entries.push(format!("{name}/"));
            } else {
                entries.push(format!("{name} ({} bytes)", meta.len()));
            }
        }
        entries.sort();
        let mut text = i18n::t_args(
            "Directory: {path}\n",
            &[("path", dir.display().to_string())],
        );
        for entry in entries.iter().take(300) {
            text.push_str(entry);
            text.push('\n');
        }
        if entries.len() > 300 {
            text.push_str(&i18n::t_args(
                "… ({total} entries total; showing first 300)\n",
                &[("total", entries.len().to_string())],
            ));
        }
        if entries.is_empty() {
            text.push_str(&i18n::t("(empty directory)\n"));
        }
        Ok(text)
    }

    fn fs_read(&self, args: &Value, workspace: &Path) -> Result<String, String> {
        let requested = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| i18n::t("Missing path argument"))?;
        let path = self.sandbox.resolve_path(workspace, requested)?;
        let meta = fs::metadata(&path).map_err(|e| {
            i18n::t_args("Failed to read file: {error}", &[("error", e.to_string())])
        })?;
        if !meta.is_file() {
            return Err(i18n::t_args(
                "Not a file: {path}",
                &[("path", requested.to_string())],
            ));
        }
        if meta.len() > 1_500_000 {
            return Err(i18n::t(
                "File is too large (>1.5 MB); use shell_exec to read it in chunks",
            ));
        }
        let bytes = fs::read(&path).map_err(|e| {
            i18n::t_args("Failed to read file: {error}", &[("error", e.to_string())])
        })?;
        if bytes.iter().take(4096).any(|b| *b == 0) {
            return Err(i18n::t("Binary file detected; refusing to read it as text"));
        }
        let content = String::from_utf8_lossy(&bytes);
        let offset = args
            .get("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(1)
            .max(1) as usize;
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(400)
            .clamp(1, 2000) as usize;
        let lines: Vec<&str> = content.lines().collect();
        let total = lines.len();
        let start = (offset - 1).min(total);
        let end = (start + limit).min(total);

        let mut text = i18n::t_args(
            "File: {path} ({total} lines)\n",
            &[
                ("path", requested.to_string()),
                ("total", total.to_string()),
            ],
        );
        for (idx, line) in lines[start..end].iter().enumerate() {
            text.push_str(&format!("{}\t{}\n", start + idx + 1, line));
        }
        if end < total {
            text.push_str(&i18n::t_args(
                "… (showing lines {start}-{end} of {total}; use offset/limit to continue)\n",
                &[
                    ("start", (start + 1).to_string()),
                    ("end", end.to_string()),
                    ("total", total.to_string()),
                ],
            ));
        }
        Ok(text)
    }

    fn fs_write(&self, args: &Value, workspace: &Path) -> Result<String, String> {
        let requested = args
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| i18n::t("Missing path argument"))?;
        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| i18n::t("Missing content argument"))?;
        if content.len() > 2_000_000 {
            return Err(i18n::t("Content is too large (>2 MB)"));
        }
        let path = self.sandbox.resolve_path(workspace, requested)?;

        if args
            .get("create_dirs")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    i18n::t_args(
                        "Failed to create directory: {error}",
                        &[("error", e.to_string())],
                    )
                })?;
            }
        } else if path.parent().map(|p| !p.exists()).unwrap_or(false) {
            return Err(i18n::t(
                "Parent directory does not exist; pass create_dirs=true to create it",
            ));
        }

        let mut backup_note = String::new();
        if path.exists() {
            let old = fs::read(&path).map_err(|e| {
                i18n::t_args(
                    "Failed to read previous content: {error}",
                    &[("error", e.to_string())],
                )
            })?;
            let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S%3f");
            let safe_name = requested.replace(['/', '\\', ':'], "_");
            let backup_path = self.backup_dir.join(format!("{stamp}-{safe_name}.bak"));
            fs::write(&backup_path, &old)
                .map_err(|e| i18n::t_args("Backup failed: {error}", &[("error", e.to_string())]))?;
            backup_note = i18n::t_args(
                "Previous content backed up to {path}",
                &[("path", backup_path.display().to_string())],
            );
        }

        fs::write(&path, content.as_bytes())
            .map_err(|e| i18n::t_args("Write failed: {error}", &[("error", e.to_string())]))?;
        Ok(i18n::t_args(
            "Wrote {path} ({bytes} bytes). {note}",
            &[
                ("path", path.display().to_string()),
                ("bytes", content.len().to_string()),
                ("note", backup_note),
            ],
        ))
    }
}
