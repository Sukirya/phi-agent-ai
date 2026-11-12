//! Headless smoke tests.
//!
//! These run the real `phi-headless` binary, which is also how the desktop
//! shell is exercised in CI. The mock model makes them deterministic: no
//! network, no API key, no UI.

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};

struct TestEnv {
    data_dir: PathBuf,
    workspace: PathBuf,
}

impl TestEnv {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("phi-headless-{name}-{}", uuid::Uuid::new_v4()));
        let data_dir = root.join("data");
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        Self {
            data_dir,
            workspace,
        }
    }

    fn command(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_phi-headless"))
            .arg("--data-dir")
            .arg(&self.data_dir)
            .arg("--workspace")
            .arg(&self.workspace)
            .args(["--quiet"])
            .args(args)
            .env("ANTHROPIC_API_KEY", "")
            .env("PHI_API_KEY", "")
            .env("PHI_LANG", "en")
            .output()
            .expect("phi-headless should start")
    }

    fn run(&self, prompt: &str, extra: &[&str]) -> Value {
        let mut args = vec!["run", "--message", prompt, "--no-stream"];
        args.extend_from_slice(extra);
        let output = self.command(&args);
        assert!(
            output.status.success(),
            "run failed: status={:?}\nstdout={}\nstderr={}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        last_json(&output.stdout)
    }

    fn show(&self, session_id: &str) -> Value {
        let output = self.command(&["sessions", "show", session_id]);
        assert!(
            output.status.success(),
            "sessions show failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        last_json(&output.stdout)
    }
}

fn last_json(stdout: &[u8]) -> Value {
    let text = String::from_utf8_lossy(stdout);
    let line = text
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("no JSON line in stdout: {text}"));
    serde_json::from_str(line).unwrap_or_else(|error| panic!("bad JSON {line}: {error}"))
}

fn assistant_text(session: &Value) -> String {
    session["session"]["messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .filter(|message| message["role"] == "assistant")
                .flat_map(|message| message["blocks"].as_array().cloned().unwrap_or_default())
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str().map(str::to_string))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

#[test]
fn intelligent_ui_run_returns_openui_lang() {
    let env = TestEnv::new("ui");
    let result = env.run("show me a small dashboard", &["--ui"]);
    assert_eq!(result["run"]["state"], "Succeeded");
    assert_eq!(result["run"]["uiMode"], json!(true));

    let session = env.show(result["sessionId"].as_str().unwrap());
    let text = assistant_text(&session);
    assert!(
        text.contains("root = Card("),
        "expected an OpenUI program, got: {text}"
    );
}

#[test]
fn write_approval_allow_creates_the_file() {
    let env = TestEnv::new("allow");
    env.run("demo write", &["--approval", "allow"]);
    let path = env.workspace.join("PHI_DEMO.md");
    assert!(path.is_file(), "approved write should create {path:?}");
    let content = std::fs::read_to_string(path).unwrap();
    assert!(content.contains("# phi-agent demo"));
}

#[test]
fn write_approval_deny_leaves_the_workspace_untouched() {
    let env = TestEnv::new("deny");
    env.run("demo write", &["--approval", "deny"]);
    assert!(
        !env.workspace.join("PHI_DEMO.md").exists(),
        "denied write must not touch the workspace"
    );
}

#[test]
fn sessions_and_journal_survive_a_restart() {
    let env = TestEnv::new("restart");
    let result = env.run("inspect the workspace", &["--approval", "deny"]);
    let session_id = result["sessionId"].as_str().unwrap().to_string();

    // A fresh process (fresh Node composition) must rebuild the same session.
    let session = env.show(&session_id);
    assert_eq!(session["session"]["id"].as_str(), Some(session_id.as_str()));
    assert!(
        !session["session"]["messages"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn status_reads_a_finished_run_from_a_different_process() {
    let env = TestEnv::new("status");
    let result = env.run("inspect the workspace", &["--approval", "deny"]);
    let run_id = result["runId"].as_str().unwrap();

    let output = env.command(&["status", run_id]);
    assert!(
        output.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status = last_json(&output.stdout);
    assert_eq!(status["found"], json!(true));
    assert_eq!(status["status"]["state"], "Succeeded");
}

#[test]
fn step_limit_aborts_the_run() {
    let env = TestEnv::new("steps");
    let output = env.command(&[
        "run",
        "--message",
        "inspect the workspace",
        "--no-stream",
        "--max-steps",
        "1",
        "--approval",
        "deny",
    ]);
    assert_eq!(
        output.status.code(),
        Some(3),
        "expected aborted exit code, stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let result = last_json(&output.stdout);
    assert_eq!(result["run"]["state"], "Aborted");
}
