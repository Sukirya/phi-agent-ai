//! Headless shell.
//!
//! Runs the same `Node` as the desktop app without a UI. Durable events and
//! live model deltas go to stdout as JSONL; human summaries and logs go to
//! stderr. This is the primary way features are exercised in CI.

use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use phi_adapters::control;
use phi_adapters::jsonl_sink::{JsonlEventSink, JsonlStreamSink};
use phi_contracts::ports::RunLimits;
use phi_contracts::provider::{Pricing, ProviderInput};
use phi_contracts::stream::StreamPort;
use phi_domain::view::RunView;
use phi_i18n as i18n;
use phi_kernel::event_bus::EventSink;
use phi_node::{Node, NodeConfig, budget_limits, compose};

#[derive(Clone, Copy, Debug)]
enum ApprovalMode {
    Deny,
    Allow,
    Prompt,
}

struct GlobalOptions {
    data_dir: PathBuf,
    workspace: Option<PathBuf>,
    quiet: bool,
    language: Option<String>,
}

impl Default for GlobalOptions {
    fn default() -> Self {
        let data_dir = std::env::var_os("PHI_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(".phi")
            });
        let workspace = std::env::var_os("PHI_WORKSPACE").map(PathBuf::from);
        Self {
            data_dir,
            workspace,
            quiet: false,
            language: None,
        }
    }
}

struct RunArgs {
    message: Option<String>,
    session_id: Option<String>,
    provider: Option<String>,
    approval: ApprovalMode,
    stream: bool,
    title: Option<String>,
    limits: RunLimits,
    ui: bool,
}

enum Command {
    Run(RunArgs),
    SessionsList,
    SessionsShow(String),
    Approve {
        id: String,
        approved: bool,
        note: Option<String>,
    },
    Cancel(String),
    Status(String),
    Bootstrap,
    ProvidersList,
    ProvidersShow(String),
    ProvidersAdd {
        input: ProviderInput,
        activate: bool,
    },
    ProvidersUpdate {
        input: ProviderInput,
        activate: bool,
    },
    ProvidersRemove(String),
    ProvidersUse(String),
    ProvidersTest(Option<String>),
    ProvidersModels(Option<String>),
    Help,
}

#[tokio::main]
async fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    // Resolve the language before parsing so even usage errors are localized.
    let language = scan_lang(&raw);
    i18n::set_lang(i18n::resolve(language.as_deref(), None));
    let (global, command) = match parse_cli(raw) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("[phi-headless] {error}");
            eprintln!();
            print_help();
            std::process::exit(2);
        }
    };
    let code = match execute(global, command).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("[phi-headless] error: {error}");
            2
        }
    };
    std::process::exit(code);
}

async fn execute(global: GlobalOptions, command: Command) -> Result<i32, String> {
    match command {
        Command::Help => {
            print_help();
            Ok(0)
        }
        Command::Run(args) => run(global, args).await,
        Command::SessionsList => {
            let node = open_node(&global, false)?;
            let sessions = node.list_sessions();
            print_json(&serde_json::json!({
                "type": "sessions",
                "sessions": sessions,
            }))?;
            Ok(0)
        }
        Command::SessionsShow(id) => {
            let node = open_node(&global, false)?;
            print_json(&node.get_session(&id)?)?;
            Ok(0)
        }
        Command::Approve { id, approved, note } => {
            // The run lives in another process; hand the decision over through
            // the data directory instead of composing a second writer.
            control::request_approval(&global.data_dir, &id, approved, note)?;
            print_json(&serde_json::json!({
                "type": "approval",
                "approvalId": id,
                "approved": approved,
                "queued": true,
            }))?;
            Ok(0)
        }
        Command::Cancel(run_id) => {
            control::request_cancel(&global.data_dir, &run_id)?;
            print_json(&serde_json::json!({
                "type": "cancel",
                "runId": run_id,
                "cancelled": true,
                "queued": true,
            }))?;
            Ok(0)
        }
        Command::Status(run_id) => {
            let node = open_node(&global, false)?;
            let status = node.run_status(&run_id);
            print_json(&serde_json::json!({
                "type": "run_status",
                "runId": run_id,
                "found": status.is_some(),
                "status": status,
            }))?;
            Ok(0)
        }
        Command::Bootstrap => {
            let node = open_node(&global, true)?;
            print_json(&node.bootstrap()?)?;
            Ok(0)
        }
        Command::ProvidersList => {
            let node = open_node(&global, false)?;
            print_json(&serde_json::json!({
                "type": "providers",
                "providers": node.provider_views(),
                "activeProviderId": node.active_provider_id(),
            }))?;
            Ok(0)
        }
        Command::ProvidersShow(id) => {
            let node = open_node(&global, false)?;
            let view = node
                .provider_view(&id)
                .ok_or_else(|| i18n::t_args("Unknown provider: {id}", &[("id", id.clone())]))?;
            print_json(&view)?;
            Ok(0)
        }
        Command::ProvidersAdd { input, activate } => {
            let node = open_node(&global, true)?;
            let view = node.save_provider(input)?;
            if activate {
                node.set_active_provider(&view.id)?;
            }
            print_json(&view)?;
            Ok(0)
        }
        Command::ProvidersUpdate { input, activate } => {
            let node = open_node(&global, true)?;
            let view = node.save_provider(input)?;
            if activate {
                node.set_active_provider(&view.id)?;
            }
            print_json(&view)?;
            Ok(0)
        }
        Command::ProvidersRemove(id) => {
            let node = open_node(&global, true)?;
            let removed = node.remove_provider(&id)?;
            print_json(&serde_json::json!({
                "type": "provider_removed",
                "providerId": id,
                "removed": removed,
            }))?;
            Ok(0)
        }
        Command::ProvidersUse(id) => {
            let node = open_node(&global, true)?;
            node.set_active_provider(&id)?;
            print_json(&serde_json::json!({
                "type": "provider_active",
                "providerId": id,
            }))?;
            Ok(0)
        }
        Command::ProvidersTest(id) => {
            let node = open_node(&global, true)?;
            let result = node.test_provider(id.as_deref()).await;
            let ok = result.ok;
            print_json(&result)?;
            Ok(if ok { 0 } else { 4 })
        }
        Command::ProvidersModels(id) => {
            let node = open_node(&global, true)?;
            let models = node.fetch_models(id.as_deref()).await?;
            print_json(&serde_json::json!({
                "type": "models",
                "models": models,
            }))?;
            Ok(0)
        }
    }
}

fn open_node(global: &GlobalOptions, writable: bool) -> Result<Arc<Node>, String> {
    compose(NodeConfig {
        data_dir: global.data_dir.clone(),
        workspace: global.workspace.clone(),
        fallback_workspace: None,
        event_sinks: Vec::new(),
        stream_sink: None,
        language: global.language.clone(),
        writable,
    })
}

async fn run(global: GlobalOptions, args: RunArgs) -> Result<i32, String> {
    let event_sink: Arc<dyn EventSink> = if global.quiet {
        Arc::new(JsonlEventSink::quiet())
    } else {
        Arc::new(JsonlEventSink::new())
    };
    let stream_sink: Option<Arc<dyn StreamPort>> = if args.stream {
        Some(Arc::new(JsonlStreamSink))
    } else {
        None
    };
    let node = compose(NodeConfig {
        data_dir: global.data_dir.clone(),
        workspace: global.workspace.clone(),
        fallback_workspace: None,
        event_sinks: vec![event_sink],
        stream_sink,
        language: global.language.clone(),
        writable: true,
    })?;

    if let Some(provider) = args.provider.as_deref() {
        node.set_active_provider(provider)?;
    }

    let message = match args.message {
        Some(message) if message.trim() == "-" => read_stdin()?,
        Some(message) => message,
        None => read_stdin()?,
    };
    let message = message.trim().to_string();
    if message.is_empty() {
        return Err(i18n::t(
            "Message is empty (use --message or pipe text through stdin)",
        ));
    }

    let session_id = match args.session_id {
        Some(id) => {
            node.get_session(&id)?;
            id
        }
        None => {
            let title = args
                .title
                .unwrap_or_else(|| message.chars().take(24).collect::<String>());
            node.create_session(Some(&title))?.id
        }
    };

    let worker = tokio::spawn(approval_worker(
        node.clone(),
        session_id.clone(),
        args.approval,
    ));
    let run_id = node.send_message_full(&session_id, &message, None, args.limits, args.ui);
    let run_id = match run_id {
        Ok(run_id) => run_id,
        Err(error) => {
            worker.abort();
            return Err(error);
        }
    };

    let wall_secs = args
        .limits
        .max_wall_secs
        .unwrap_or_else(|| budget_limits().2);
    let timeout = Duration::from_secs(wall_secs.saturating_add(30));
    let result = wait_for_terminal(&node, &session_id, timeout).await;
    worker.abort();
    let run = result?;

    print_json(&serde_json::json!({
        "type": "result",
        "sessionId": session_id,
        "runId": run_id,
        "run": run,
    }))?;
    eprintln!(
        "[phi-headless] run {} {} steps={} cost=${:.4}{}",
        run.id,
        run.state,
        run.steps,
        run.cost_usd,
        run.error
            .as_deref()
            .map(|error| format!(" ({error})"))
            .unwrap_or_default()
    );

    Ok(match run.state.as_str() {
        "Succeeded" => 0,
        "Aborted" => 3,
        "AwaitingReconciliation" => 5,
        _ => 4,
    })
}

async fn wait_for_terminal(
    node: &Node,
    session_id: &str,
    timeout: Duration,
) -> Result<RunView, String> {
    let started = Instant::now();
    loop {
        if let Some(session) = node.session_view(session_id) {
            if let Some(run) = session.runs.last() {
                if matches!(
                    run.state.as_str(),
                    "Succeeded" | "Failed" | "Aborted" | "AwaitingReconciliation"
                ) {
                    return Ok(run.clone());
                }
            }
        }
        if started.elapsed() >= timeout {
            return Err(i18n::t_args(
                "Timed out waiting for the run to finish ({secs}s)",
                &[("secs", timeout.as_secs().to_string())],
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn approval_worker(node: Arc<Node>, session_id: String, mode: ApprovalMode) {
    loop {
        for approval in node.pending_approvals(&session_id) {
            let (approved, note) = match mode {
                ApprovalMode::Deny => (false, "headless: denied by --approval deny".to_string()),
                ApprovalMode::Allow => (
                    true,
                    "headless: auto-approved by --approval allow (tests only)".to_string(),
                ),
                ApprovalMode::Prompt => {
                    eprintln!(
                        "[approval] {} tool={} risk={}",
                        approval.id, approval.tool_id, approval.risk
                    );
                    eprintln!("[approval] args={}", approval.args_preview);
                    eprintln!("[approval] approve? [y/N]");
                    let line = tokio::task::spawn_blocking(prompt_line)
                        .await
                        .unwrap_or_default();
                    let approved = matches!(
                        line.trim().to_ascii_lowercase().as_str(),
                        "y" | "yes" | "approve"
                    );
                    (
                        approved,
                        format!(
                            "headless: interactive ({})",
                            if approved { "approved" } else { "rejected" }
                        ),
                    )
                }
            };
            if let Err(error) = node.approve(&approval.id, approved, Some(note)) {
                eprintln!(
                    "[phi-headless] {}",
                    i18n::t_args("Approval failed: {error}", &[("error", error)])
                );
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn prompt_line() -> String {
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    line
}

fn read_stdin() -> Result<String, String> {
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|error| {
            i18n::t_args(
                "Failed to read stdin: {error}",
                &[("error", error.to_string())],
            )
        })?;
    Ok(text)
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<(), String> {
    let text = serde_json::to_string(value).map_err(|error| error.to_string())?;
    println!("{text}");
    Ok(())
}

// ─────────────────────────── argument parsing ───────────────────────────

/// Find `--lang` before the regular parser so usage errors are localized.
fn scan_lang(raw: &[String]) -> Option<String> {
    let mut index = 0;
    while index < raw.len() {
        let arg = &raw[index];
        if arg == "--lang" {
            return raw.get(index + 1).cloned();
        }
        if let Some(value) = arg.strip_prefix("--lang=") {
            return Some(value.to_string());
        }
        index += 1;
    }
    None
}

fn parse_cli(raw: Vec<String>) -> Result<(GlobalOptions, Command), String> {
    let mut global = GlobalOptions::default();
    let mut remaining = Vec::new();
    let mut index = 0;
    while index < raw.len() {
        let arg = &raw[index];
        match arg.as_str() {
            "--help" | "-h" => return Ok((global, Command::Help)),
            "--quiet" => {
                global.quiet = true;
                index += 1;
            }
            "--json" => {
                // stdout is already JSONL; accepted for compatibility.
                index += 1;
            }
            "--data-dir" => {
                index += 1;
                global.data_dir = PathBuf::from(
                    raw.get(index)
                        .ok_or_else(|| i18n::t("--data-dir requires a value"))?,
                );
                index += 1;
            }
            "--workspace" => {
                index += 1;
                global.workspace = Some(PathBuf::from(
                    raw.get(index)
                        .ok_or_else(|| i18n::t("--workspace requires a value"))?,
                ));
                index += 1;
            }
            "--lang" => {
                index += 1;
                global.language = Some(
                    raw.get(index)
                        .cloned()
                        .ok_or_else(|| i18n::t("--lang requires a value"))?,
                );
                index += 1;
            }
            _ if arg.starts_with("--data-dir=") => {
                global.data_dir = PathBuf::from(arg.trim_start_matches("--data-dir="));
                index += 1;
            }
            _ if arg.starts_with("--workspace=") => {
                global.workspace = Some(PathBuf::from(arg.trim_start_matches("--workspace=")));
                index += 1;
            }
            _ if arg.starts_with("--lang=") => {
                global.language = Some(arg.trim_start_matches("--lang=").to_string());
                index += 1;
            }
            _ => {
                remaining.push(arg.clone());
                index += 1;
            }
        }
    }

    let Some(command) = remaining.first().cloned() else {
        return Ok((global, Command::Help));
    };
    let rest = &remaining[1..];
    let command = match command.as_str() {
        "run" => Command::Run(parse_run(rest)?),
        "sessions" => match rest.first().map(String::as_str) {
            Some("list") | Some("ls") | None => Command::SessionsList,
            Some("show") => Command::SessionsShow(parse_id(rest, 1)?),
            Some(other) => {
                return Err(i18n::t_args(
                    "Unknown sessions subcommand: {command}",
                    &[("command", other.to_string())],
                ));
            }
        },
        "approve" => parse_approve(rest)?,
        "cancel" => Command::Cancel(parse_id(rest, 0)?),
        "status" => Command::Status(parse_id(rest, 0)?),
        "bootstrap" => Command::Bootstrap,
        "providers" => parse_providers(rest)?,
        other => {
            return Err(i18n::t_args(
                "Unknown command: {command}",
                &[("command", other.to_string())],
            ));
        }
    };
    Ok((global, command))
}

fn parse_run(rest: &[String]) -> Result<RunArgs, String> {
    let mut args = RunArgs {
        message: None,
        session_id: None,
        provider: None,
        approval: ApprovalMode::Deny,
        stream: true,
        title: None,
        limits: RunLimits::default(),
        ui: false,
    };
    let mut index = 0;
    while index < rest.len() {
        if let Some(value) = take_flag(rest, &mut index, "--message")? {
            args.message = Some(value);
        } else if let Some(value) = take_flag(rest, &mut index, "--session")? {
            args.session_id = Some(value);
        } else if let Some(value) = take_flag(rest, &mut index, "--provider")? {
            args.provider = Some(value);
        } else if let Some(value) = take_flag(rest, &mut index, "--title")? {
            args.title = Some(value);
        } else if let Some(value) = take_flag(rest, &mut index, "--approval")? {
            args.approval = match value.trim().to_ascii_lowercase().as_str() {
                "deny" => ApprovalMode::Deny,
                "allow" | "auto" => ApprovalMode::Allow,
                "prompt" | "ask" => ApprovalMode::Prompt,
                other => {
                    return Err(i18n::t_args(
                        "Unknown approval mode {mode}; expected deny, allow or prompt",
                        &[("mode", other.to_string())],
                    ));
                }
            };
        } else if let Some(value) = take_flag(rest, &mut index, "--max-steps")? {
            args.limits.max_steps = Some(value.parse().map_err(|_| {
                i18n::t_args(
                    "--max-steps is not a number: {value}",
                    &[("value", value.clone())],
                )
            })?);
        } else if let Some(value) = take_flag(rest, &mut index, "--max-cost")? {
            args.limits.max_cost_usd = Some(value.parse().map_err(|_| {
                i18n::t_args(
                    "--max-cost is not a number: {value}",
                    &[("value", value.clone())],
                )
            })?);
        } else if let Some(value) = take_flag(rest, &mut index, "--max-wall")? {
            args.limits.max_wall_secs = Some(value.parse().map_err(|_| {
                i18n::t_args(
                    "--max-wall is not a number: {value}",
                    &[("value", value.clone())],
                )
            })?);
        } else {
            match rest[index].as_str() {
                "--no-stream" => {
                    args.stream = false;
                    index += 1;
                }
                "--stream" => {
                    args.stream = true;
                    index += 1;
                }
                "--ui" => {
                    args.ui = true;
                    index += 1;
                }
                other => {
                    return Err(i18n::t_args(
                        "Unknown run argument: {arg}",
                        &[("arg", other.to_string())],
                    ));
                }
            }
        }
    }
    Ok(args)
}

fn parse_approve(rest: &[String]) -> Result<Command, String> {
    let id = parse_id(rest, 0)?;
    let mut approved = None;
    let mut note = None;
    let mut index = 0;
    while index < rest.len() {
        let arg = &rest[index];
        if arg == "--allow" || arg == "--approve" {
            approved = Some(true);
            index += 1;
        } else if arg == "--deny" || arg == "--reject" {
            approved = Some(false);
            index += 1;
        } else if let Some(value) = take_flag(rest, &mut index, "--note")? {
            note = Some(value);
        } else if arg.as_str() == id.as_str() {
            index += 1;
        } else {
            return Err(i18n::t_args(
                "Unknown approve argument: {arg}",
                &[("arg", arg.clone())],
            ));
        }
    }
    let approved = approved.ok_or_else(|| i18n::t("approve requires --allow or --deny"))?;
    Ok(Command::Approve { id, approved, note })
}

fn parse_id(rest: &[String], position: usize) -> Result<String, String> {
    if let Some(value) = rest.get(position) {
        if !value.starts_with('-') {
            return Ok(value.clone());
        }
    }
    let mut index = 0;
    while index < rest.len() {
        if let Some(value) = take_flag(rest, &mut index, "--id")? {
            return Ok(value);
        }
        index += 1;
    }
    Err(i18n::t("Missing id"))
}

fn parse_providers(rest: &[String]) -> Result<Command, String> {
    let Some(sub) = rest.first().map(String::as_str) else {
        return Ok(Command::ProvidersList);
    };
    let args = &rest[1..];
    match sub {
        "list" | "ls" => Ok(Command::ProvidersList),
        "show" => Ok(Command::ProvidersShow(parse_id(args, 0)?)),
        "add" => {
            let (input, activate) = parse_provider_input(args, false)?;
            Ok(Command::ProvidersAdd { input, activate })
        }
        "update" | "edit" => {
            let (input, activate) = parse_provider_input(args, true)?;
            Ok(Command::ProvidersUpdate { input, activate })
        }
        "remove" | "rm" => Ok(Command::ProvidersRemove(parse_id(args, 0)?)),
        "use" | "activate" => Ok(Command::ProvidersUse(parse_id(args, 0)?)),
        "test" => Ok(Command::ProvidersTest(optional_id(args)?)),
        "models" => Ok(Command::ProvidersModels(optional_id(args)?)),
        other => Err(i18n::t_args(
            "Unknown providers subcommand: {command}",
            &[("command", other.to_string())],
        )),
    }
}

fn optional_id(rest: &[String]) -> Result<Option<String>, String> {
    match parse_id(rest, 0) {
        Ok(id) => Ok(Some(id)),
        Err(_) => Ok(None),
    }
}

fn parse_provider_input(
    rest: &[String],
    require_id: bool,
) -> Result<(ProviderInput, bool), String> {
    let mut input = ProviderInput::default();
    let mut activate = false;
    let mut positional = Vec::new();
    let mut pricing_in = None;
    let mut pricing_out = None;
    let mut index = 0;
    while index < rest.len() {
        if let Some(value) = take_flag(rest, &mut index, "--id")? {
            input.id = Some(value);
        } else if let Some(value) = take_flag(rest, &mut index, "--name")? {
            input.name = Some(value);
        } else if let Some(value) = take_flag(rest, &mut index, "--protocol")? {
            input.protocol = Some(value);
        } else if let Some(value) = take_flag(rest, &mut index, "--base-url")? {
            input.base_url = Some(value);
        } else if let Some(value) = take_flag(rest, &mut index, "--api-key")? {
            input.api_key = Some(value);
        } else if let Some(value) = take_flag(rest, &mut index, "--model")? {
            input.model = Some(value);
        } else if let Some(value) = take_flag(rest, &mut index, "--auth")? {
            input.auth = Some(value);
        } else if let Some(value) = take_flag(rest, &mut index, "--models")? {
            input.models = Some(
                value
                    .split(',')
                    .map(|item| item.trim().to_string())
                    .filter(|item| !item.is_empty())
                    .collect(),
            );
        } else if let Some(value) = take_flag(rest, &mut index, "--model-map")? {
            let mut map = std::collections::HashMap::new();
            for pair in value.split(',') {
                let Some((key, value)) = pair.split_once('=') else {
                    return Err(i18n::t_args(
                        "--model-map expects key=model: {pair}",
                        &[("pair", pair.to_string())],
                    ));
                };
                map.insert(key.trim().to_string(), value.trim().to_string());
            }
            input.model_map = Some(map);
        } else if let Some(value) = take_flag(rest, &mut index, "--pricing-in")? {
            pricing_in = Some(value.parse::<f64>().map_err(|_| {
                i18n::t_args(
                    "--pricing-in is not a number: {value}",
                    &[("value", value.clone())],
                )
            })?);
        } else if let Some(value) = take_flag(rest, &mut index, "--pricing-out")? {
            pricing_out = Some(value.parse::<f64>().map_err(|_| {
                i18n::t_args(
                    "--pricing-out is not a number: {value}",
                    &[("value", value.clone())],
                )
            })?);
        } else if let Some(value) = take_flag(rest, &mut index, "--max-output-tokens")? {
            input.max_output_tokens = Some(value.parse().map_err(|_| {
                i18n::t_args(
                    "--max-output-tokens is not a number: {value}",
                    &[("value", value.clone())],
                )
            })?);
        } else if let Some(value) = take_flag(rest, &mut index, "--temperature")? {
            input.temperature = Some(value.parse().map_err(|_| {
                i18n::t_args(
                    "--temperature is not a number: {value}",
                    &[("value", value.clone())],
                )
            })?);
        } else if let Some(value) = take_flag(rest, &mut index, "--sort-index")? {
            input.sort_index = Some(value.parse().map_err(|_| {
                i18n::t_args(
                    "--sort-index is not a number: {value}",
                    &[("value", value.clone())],
                )
            })?);
        } else if let Some(value) = take_flag(rest, &mut index, "--icon")? {
            input.icon = Some(value);
        } else if let Some(value) = take_flag(rest, &mut index, "--icon-color")? {
            input.icon_color = Some(value);
        } else if let Some(value) = take_flag(rest, &mut index, "--website")? {
            input.website_url = Some(value);
        } else if let Some(value) = take_flag(rest, &mut index, "--notes")? {
            input.notes = Some(value);
        } else {
            match rest[index].as_str() {
                "--activate" => {
                    activate = true;
                    index += 1;
                }
                "--disabled" => {
                    input.enabled = Some(false);
                    index += 1;
                }
                value if !value.starts_with('-') => {
                    positional.push(value.to_string());
                    index += 1;
                }
                other => {
                    return Err(i18n::t_args(
                        "Unknown provider argument: {arg}",
                        &[("arg", other.to_string())],
                    ));
                }
            }
        }
    }
    if input.id.is_none() {
        if let Some(first) = positional.first() {
            input.id = Some(first.clone());
        }
    }
    if require_id && input.id.is_none() {
        return Err(i18n::t("update requires a provider id"));
    }
    if pricing_in.is_some() || pricing_out.is_some() {
        input.pricing = Some(Pricing {
            input_per_mtok: pricing_in.unwrap_or(0.0),
            output_per_mtok: pricing_out.unwrap_or(0.0),
        });
    }
    Ok((input, activate))
}

fn take_flag(args: &[String], index: &mut usize, flag: &str) -> Result<Option<String>, String> {
    let arg = args.get(*index).ok_or_else(|| {
        i18n::t_args(
            "Internal error: index out of bounds ({flag})",
            &[("flag", flag.to_string())],
        )
    })?;
    if arg == flag {
        let value = args.get(*index + 1).cloned().ok_or_else(|| {
            i18n::t_args("{flag} requires a value", &[("flag", flag.to_string())])
        })?;
        *index += 2;
        return Ok(Some(value));
    }
    let prefix = format!("{flag}=");
    if let Some(value) = arg.strip_prefix(&prefix) {
        *index += 1;
        return Ok(Some(value.to_string()));
    }
    Ok(None)
}

fn print_help() {
    eprintln!(
        r#"phi-headless — local-first agent node without a UI

Usage:
  phi-headless [global options] <command> [options]

Commands:
  run --message TEXT [--session ID] [--provider ID]
      [--approval deny|allow|prompt] [--no-stream] [--title TEXT]
      [--max-steps N] [--max-cost USD] [--max-wall SECS] [--ui]
  sessions list
  sessions show <id>
  approve <id> --allow|--deny [--note TEXT]
  cancel <run-id>
  status <run-id>
  bootstrap
  providers list
  providers show <id>
  providers add --name NAME --protocol P --base-url URL --model MODEL
      [--api-key env:NAME|literal:VALUE|VALUE] [--auth x_api_key|bearer|none]
      [--pricing-in N] [--pricing-out N] [--max-output-tokens N]
      [--temperature N] [--models a,b] [--model-map role=model]
      [--icon NAME] [--icon-color #hex] [--website URL] [--notes TEXT]
      [--sort-index N] [--activate] [--disabled]
  providers update <id> [same flags]
  providers remove <id>
  providers use <id>
  providers test [id]
  providers models [id]

Global options:
  --data-dir DIR    default $PHI_DATA_DIR or ./.phi
  --workspace DIR   default $PHI_WORKSPACE or <data-dir>/workspace
  --lang LANG       en or zh-CN (default $PHI_LANG, then saved setting)
  --quiet           suppress per-event JSON lines on stdout
  --json            accepted; stdout is already JSONL

stdout: one JSON object per line (events, model deltas, final result)
stderr: logs and human summaries
exit:   0 success, 3 aborted, 4 failed, 5 needs reconciliation, 2 usage/config error"#
    );
}
