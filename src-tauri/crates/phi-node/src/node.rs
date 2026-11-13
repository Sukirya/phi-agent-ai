//! Shared node composition and application services.
//!
//! This crate is the shell-independent core: the desktop shell and the
//! headless CLI both compose the same `Node` and call the same methods. No
//! Tauri type may appear here.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::Serialize;

use phi_adapters::config::ConfigStore;
use phi_adapters::control;
use phi_adapters::fs_blob::FsBlobStore;
use phi_adapters::fs_journal::JsonlJournal;
use phi_adapters::lock::DataDirLock;
use phi_adapters::system::{StderrLog, SystemClock, UuidIds};
use phi_contracts::ports::{ModelStatus, RunLimits, RunStatus, SessionPort, StartRunRequest};
use phi_contracts::provider::{ProviderInput, ProviderTestView, ProviderView};
use phi_contracts::services::Services;
use phi_contracts::stream::{StreamHub, StreamPort};
use phi_contracts::{REQUIRED_SLOTS, SLOT_PERSISTENCE_JOURNAL};
use phi_domain::message::{ContentBlock, Role};
use phi_domain::view::{PendingApprovalView, SessionSummary, SessionView};
use phi_i18n as i18n;
use phi_kernel::event_bus::{EventBus, EventSink};
use phi_kernel::lifecycle::{Kernel, RunMode};
use phi_kernel::ports::LogPort;
use phi_kernel::registry::Registry;
use phi_plugins::approver::HumanApprover;
use phi_plugins::context::DefaultContextPlugin;
use phi_plugins::loop_plugin::LoopPlugin;
use phi_plugins::model_gateway::ModelGateway;
use phi_plugins::policy::DefaultPolicyPlugin;
use phi_plugins::provider_store::ProviderStore;
use phi_plugins::sandbox::FsSandboxPlugin;
use phi_plugins::session::SessionPlugin;
use phi_plugins::store::StorePlugin;
use phi_plugins::tools::BuiltinToolsPlugin;

#[derive(Clone)]
pub struct NodeConfig {
    pub data_dir: PathBuf,
    /// Explicit workspace selection (headless `--workspace`). Wins over the
    /// persisted setting and the fallback.
    pub workspace: Option<PathBuf>,
    /// Used when no workspace has been persisted yet. The desktop shell passes
    /// `Documents/phi-workspace`; headless defaults to `<data_dir>/workspace`.
    pub fallback_workspace: Option<PathBuf>,
    /// Shell-specific event sinks (Tauri emitter, JSONL writer, test collector).
    pub event_sinks: Vec<Arc<dyn EventSink>>,
    /// Shell-specific live stream sink. Without one the gateway stays
    /// non-streaming.
    pub stream_sink: Option<Arc<dyn StreamPort>>,
    /// Per-process language override (CLI `--lang`). `None` falls back to
    /// `PHI_LANG`, the persisted setting, then the OS locale.
    pub language: Option<String>,
    /// Writable nodes take the data-directory lock and may append to the
    /// journal or persist settings. Read-only shells (for example
    /// `phi-headless status`) only project the existing log.
    pub writable: bool,
}

pub struct Node {
    node_id: String,
    bus: Arc<EventBus>,
    kernel: Arc<Kernel>,
    services: Arc<Services>,
    config: Arc<ConfigStore>,
    providers: Arc<ProviderStore>,
    model_gateway: Arc<ModelGateway>,
    data_dir: PathBuf,
    writable: bool,
    // Stop the watcher before releasing the data-directory lock.
    _control_watcher: Option<ControlWatcher>,
    _data_lock: Option<DataDirLock>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bootstrap {
    pub node_id: String,
    pub mode: String,
    pub mode_reason: String,
    pub model: ModelStatus,
    pub workspace: Option<String>,
    pub sessions: Vec<SessionSummary>,
    pub last_session_id: Option<String>,
    pub max_steps: u64,
    pub max_cost_usd: f64,
    pub max_wall_secs: u64,
    pub providers: Vec<ProviderView>,
    pub active_provider_id: Option<String>,
    pub language: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetail {
    pub session: SessionView,
    pub pending_approvals: Vec<PendingApprovalView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendResult {
    pub run_id: String,
}

/// Applies approval/cancel command files written by another shell process.
struct ControlWatcher {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for ControlWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub fn compose(config: NodeConfig) -> Result<Arc<Node>, String> {
    // Acquire the single-writer lock before touching any persistent file.
    let data_lock = if config.writable {
        Some(DataDirLock::acquire(&config.data_dir)?)
    } else {
        None
    };

    let journal = Arc::new(JsonlJournal::new(
        config.data_dir.join("journal").join("events.jsonl"),
    ));
    let blob = Arc::new(FsBlobStore::new(config.data_dir.join("blobs")));
    let clock = Arc::new(SystemClock::new());
    let ids = Arc::new(UuidIds);
    let log = Arc::new(StderrLog);
    let app_config = Arc::new(ConfigStore::load(config.data_dir.join("config.json")));

    let configured_language = app_config.snapshot().language;
    let language = i18n::resolve(config.language.as_deref(), configured_language.as_deref());
    i18n::set_lang(language);
    if config.writable
        && configured_language.is_none()
        && config.language.is_none()
        && std::env::var("PHI_LANG").is_err()
    {
        let code = language.code().to_string();
        app_config.update(|config| config.language = Some(code))?;
    }

    let node_id = if config.writable {
        app_config.update(|config| {
            if config.node_id.is_none() {
                config.node_id = Some(format!("node_{}", uuid::Uuid::new_v4().simple()));
            }
            config.node_id.clone().unwrap_or_default()
        })?
    } else {
        app_config
            .snapshot()
            .node_id
            .unwrap_or_else(|| "node_readonly".to_string())
    };

    let workspace = resolve_workspace(&config, &app_config, &log)?;
    log.info(&format!("workspace: {}", workspace.display()));

    let providers = Arc::new(ProviderStore::load(config.data_dir.join("providers.json")));
    if config.writable {
        if let Err(error) = providers.seed_from_env() {
            log.warn(&i18n::t_args(
                "Failed to import provider environment variables: {error}",
                &[("error", error)],
            ));
        }
    }

    let bus = Arc::new(EventBus::new(
        journal.clone(),
        clock.clone(),
        ids.clone(),
        log.clone(),
    ));
    if !config.writable {
        bus.set_read_only();
    }
    let records = bus.restore()?;

    let session = Arc::new(SessionPlugin::new(
        bus.clone(),
        ids.clone(),
        blob.clone(),
        node_id.clone(),
    ));
    session.load(&records);

    // Projections must observe events before any live publishing happens.
    bus.subscribe(session.clone());
    for sink in config.event_sinks {
        bus.subscribe(sink);
    }

    let streams = Arc::new(StreamHub::new(config.stream_sink));
    let sandbox = Arc::new(FsSandboxPlugin);
    let model_gateway = Arc::new(ModelGateway::new(providers.clone(), streams));
    let policy = Arc::new(DefaultPolicyPlugin::new(sandbox.clone()));
    let approver = Arc::new(HumanApprover::new(bus.clone(), ids.clone(), clock.clone()));
    let tools = Arc::new(BuiltinToolsPlugin::new(
        sandbox.clone(),
        config.data_dir.join("backups"),
    ));
    let context = Arc::new(DefaultContextPlugin::new(sandbox.clone()));

    let services = Arc::new(Services::new());
    services.set_session(session.clone());
    services.set_model(model_gateway.clone());
    services.set_policy(policy.clone());
    services.set_sandbox(sandbox.clone());
    services.set_approver(approver.clone());
    services.set_context(context.clone());
    services.add_tool_provider(tools.clone());

    let runtime = Arc::new(LoopPlugin::new(
        bus.clone(),
        ids.clone(),
        log.clone(),
        blob.clone(),
        services.clone(),
    ));
    services.set_runtime(runtime.clone());

    let registry = Arc::new(Registry::new());
    let kernel = Arc::new(Kernel::new(bus.clone(), registry.clone(), log.clone()));
    kernel.add_plugin(Arc::new(StorePlugin::new(journal.clone(), blob.clone())));
    kernel.add_plugin(sandbox.clone());
    kernel.add_plugin(model_gateway.clone());
    kernel.add_plugin(policy.clone());
    kernel.add_plugin(approver.clone());
    kernel.add_plugin(session.clone());
    kernel.add_plugin(tools.clone());
    kernel.add_plugin(context.clone());
    kernel.add_plugin(runtime.clone());

    if config.writable {
        kernel.activate_all();
        // Slot metadata and typed service wiring are two views of the same
        // composition; both must agree before the node reports Normal.
        let mut missing = services.missing_slots(REQUIRED_SLOTS);
        missing.extend(registry.missing_slots(&[SLOT_PERSISTENCE_JOURNAL]));
        if missing.is_empty() {
            kernel.set_mode(
                RunMode::Normal,
                &i18n::t("All required slots are available"),
            );
        } else {
            kernel.set_mode(
                RunMode::Maintenance,
                &i18n::t_args(
                    "Required services are missing: {slots}",
                    &[("slots", missing.join(", "))],
                ),
            );
        }
    } else {
        // Read-only shells never activate plugins: activation probes storage,
        // and a probe must not write while another process owns the journal.
        kernel.set_initial_mode(RunMode::Normal, &i18n::t("Read-only shell"));
    }

    if config.writable {
        let interrupted = session.recover_interrupted();
        if interrupted > 0 {
            log.warn(&format!(
                "{interrupted} interrupted run(s) marked AwaitingReconciliation"
            ));
        }
        control::clean_stale(&config.data_dir, Duration::from_secs(24 * 60 * 60));
    }

    let control_watcher = if config.writable {
        Some(spawn_control_watcher(
            services.clone(),
            config.data_dir.clone(),
        ))
    } else {
        None
    };

    Ok(Arc::new(Node {
        node_id,
        bus,
        kernel,
        services,
        config: app_config,
        providers,
        model_gateway,
        data_dir: config.data_dir,
        writable: config.writable,
        _data_lock: data_lock,
        _control_watcher: control_watcher,
    }))
}

fn spawn_control_watcher(services: Arc<Services>, data_dir: PathBuf) -> ControlWatcher {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = stop.clone();
    let handle = std::thread::spawn(move || {
        while !stop_for_thread.load(Ordering::Relaxed) {
            for command in control::take_cancels(&data_dir) {
                if let Some(runtime) = services.runtime() {
                    runtime.cancel(&command.run_id, &i18n::t("Cancelled by user"));
                }
            }
            for command in control::take_approvals(&data_dir) {
                if let Some(approver) = services.approver() {
                    approver.resolve(&command.approval_id, command.approved, command.note);
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    });
    ControlWatcher {
        stop,
        handle: Some(handle),
    }
}

fn resolve_workspace(
    config: &NodeConfig,
    app_config: &ConfigStore,
    log: &Arc<StderrLog>,
) -> Result<PathBuf, String> {
    // 1. An explicit shell selection always wins.
    if let Some(path) = &config.workspace {
        let workspace = path.clone();
        persist_workspace(config, app_config, &workspace)?;
        log.info(&format!("workspace (explicit): {}", workspace.display()));
        return Ok(workspace);
    }
    // 2. The saved setting, when it still exists.
    if let Some(path) = app_config.snapshot().workspace {
        let candidate = PathBuf::from(&path);
        if candidate.is_dir() {
            return Ok(candidate);
        }
    }
    // 3. Shell default, then the data-directory default.
    let workspace = match &config.fallback_workspace {
        Some(path) => path.clone(),
        None => config.data_dir.join("workspace"),
    };
    persist_workspace(config, app_config, &workspace)?;
    if config.writable {
        log.info(&format!(
            "default workspace created: {}",
            workspace.display()
        ));
    }
    Ok(workspace)
}

fn persist_workspace(
    config: &NodeConfig,
    app_config: &ConfigStore,
    workspace: &Path,
) -> Result<(), String> {
    if !config.writable {
        return Ok(());
    }
    std::fs::create_dir_all(workspace).map_err(|error| {
        i18n::t_args(
            "Failed to create workspace: {error}",
            &[("error", error.to_string())],
        )
    })?;
    let display = workspace.to_string_lossy().to_string();
    app_config.update(|config| config.workspace = Some(display))?;
    Ok(())
}

impl Node {
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn writable(&self) -> bool {
        self.writable
    }

    pub fn bus(&self) -> &Arc<EventBus> {
        &self.bus
    }

    fn require_writable(&self) -> Result<(), String> {
        if self.writable {
            Ok(())
        } else {
            Err(i18n::t("The node is read-only"))
        }
    }

    fn session_port(&self) -> Result<Arc<dyn SessionPort>, String> {
        self.services
            .session()
            .ok_or_else(|| i18n::t("Session store is not available"))
    }

    pub fn mode(&self) -> RunMode {
        self.kernel.mode()
    }

    pub fn mode_reason(&self) -> String {
        self.kernel.mode_reason()
    }

    pub fn bootstrap(&self) -> Result<Bootstrap, String> {
        let mut sessions = self.list_sessions();
        if sessions.is_empty() && self.writable {
            self.create_session(Some(&i18n::t("New session")))?;
            sessions = self.list_sessions();
        }
        let model = self.model_status();
        let (max_steps, max_cost_usd, max_wall_secs) = budget_limits();
        Ok(Bootstrap {
            node_id: self.node_id.clone(),
            mode: self.kernel.mode().as_str().to_string(),
            mode_reason: self.kernel.mode_reason(),
            model,
            workspace: self.config.snapshot().workspace,
            last_session_id: sessions.first().map(|session| session.id.clone()),
            sessions,
            max_steps,
            max_cost_usd,
            max_wall_secs,
            providers: self.provider_views(),
            active_provider_id: self.active_provider_id(),
            language: i18n::current().code().to_string(),
        })
    }

    pub fn list_sessions(&self) -> Vec<SessionSummary> {
        self.services
            .session()
            .map(|session| session.list_sessions())
            .unwrap_or_default()
    }

    pub fn session_view(&self, session_id: &str) -> Option<SessionView> {
        self.services
            .session()
            .and_then(|session| session.get_session(session_id))
    }

    pub fn pending_approvals(&self, session_id: &str) -> Vec<PendingApprovalView> {
        self.services
            .approver()
            .map(|approver| approver.pending_for_session(session_id))
            .unwrap_or_default()
    }

    pub fn provider_views(&self) -> Vec<ProviderView> {
        self.providers.views()
    }

    pub fn provider_view(&self, id: &str) -> Option<ProviderView> {
        self.providers.view(id)
    }

    pub fn active_provider_id(&self) -> Option<String> {
        self.providers.active_id()
    }

    pub fn set_language(&self, code: &str) -> Result<String, String> {
        self.require_writable()?;
        let lang = i18n::Lang::parse(code).ok_or_else(|| {
            i18n::t_args(
                "Unknown language {lang}; expected en or zh-CN",
                &[("lang", code.to_string())],
            )
        })?;
        i18n::set_lang(lang);
        let code = lang.code().to_string();
        self.config
            .update(|config| config.language = Some(code.clone()))?;
        Ok(code)
    }

    pub fn set_workspace(&self, path: &str) -> Result<String, String> {
        self.require_writable()?;
        let canonical = PathBuf::from(path.trim()).canonicalize().map_err(|error| {
            i18n::t_args(
                "Path is not available: {error}",
                &[("error", error.to_string())],
            )
        })?;
        if !canonical.is_dir() {
            return Err(i18n::t("Workspace must be a directory"));
        }
        let display = canonical.to_string_lossy().to_string();
        self.config
            .update(|config| config.workspace = Some(display.clone()))?;
        Ok(display)
    }

    pub fn create_session(&self, title: Option<&str>) -> Result<SessionSummary, String> {
        self.require_writable()?;
        self.session_port()?
            .create_session(title.unwrap_or(&i18n::t("New session")))
    }

    pub fn get_session(&self, session_id: &str) -> Result<SessionDetail, String> {
        let session = self.session_view(session_id).ok_or_else(|| {
            i18n::t_args("Unknown session: {id}", &[("id", session_id.to_string())])
        })?;
        let pending_approvals = self.pending_approvals(session_id);
        Ok(SessionDetail {
            session,
            pending_approvals,
        })
    }

    pub fn send_message(&self, session_id: &str, text: &str) -> Result<String, String> {
        self.send_message_with_provider(session_id, text, None)
    }

    pub fn send_message_with_provider(
        &self,
        session_id: &str,
        text: &str,
        provider_id: Option<&str>,
    ) -> Result<String, String> {
        self.send_message_full(session_id, text, provider_id, RunLimits::default(), false)
    }

    pub fn send_message_full(
        &self,
        session_id: &str,
        text: &str,
        provider_id: Option<&str>,
        limits: RunLimits,
        ui: bool,
    ) -> Result<String, String> {
        self.require_writable()?;
        if self.kernel.mode() != RunMode::Normal {
            return Err(i18n::t_args(
                "Node is in {mode} mode: {reason}",
                &[
                    ("mode", self.kernel.mode().as_str().to_string()),
                    ("reason", self.kernel.mode_reason()),
                ],
            ));
        }
        let text = text.trim();
        if text.is_empty() {
            return Err(i18n::t("Message is empty"));
        }
        let workspace = self
            .config
            .snapshot()
            .workspace
            .ok_or_else(|| i18n::t("Set a workspace first"))?;
        let workspace = PathBuf::from(workspace);
        if !workspace.is_dir() {
            return Err(i18n::t("Workspace does not exist; set it again"));
        }
        let session = self.session_port()?;
        if let Some(active) = session.active_run(session_id) {
            return Err(i18n::t_args(
                "This session still has a running task: {run_id}",
                &[("run_id", active)],
            ));
        }
        let runtime = self
            .services
            .runtime()
            .ok_or_else(|| i18n::t("Runtime is not available"))?;
        if let Some(provider_id) = provider_id {
            self.providers.set_active(provider_id)?;
        }
        let turn = session.start_turn(session_id)?;
        session.append_message(
            session_id,
            Some(&turn.turn_id),
            Role::User,
            vec![ContentBlock::text(text)],
        )?;
        runtime.start_run(StartRunRequest {
            session_id: session_id.to_string(),
            turn_id: turn.turn_id,
            workspace,
            limits,
            ui,
        })
    }

    pub fn approve(
        &self,
        approval_id: &str,
        approved: bool,
        note: Option<String>,
    ) -> Result<(), String> {
        self.require_writable()?;
        let approver = self
            .services
            .approver()
            .ok_or_else(|| i18n::t("Approval component is unavailable"))?;
        if approver.resolve(approval_id, approved, note) {
            Ok(())
        } else {
            Err(i18n::t("Approval does not exist or was already handled"))
        }
    }

    /// Cancel an in-process run, or queue the request for the process that
    /// owns it. Returns false only when the run is not active anywhere we can
    /// see.
    pub fn cancel_run(&self, run_id: &str) -> bool {
        if let Some(runtime) = self.services.runtime() {
            if runtime.cancel(run_id, &i18n::t("Cancelled by user")) {
                return true;
            }
        }
        if self.writable && control::request_cancel(&self.data_dir, run_id).is_ok() {
            return true;
        }
        false
    }

    pub fn model_status(&self) -> ModelStatus {
        match self.services.model() {
            Some(model) => model.status(),
            None => ModelStatus {
                provider: "none".to_string(),
                model: "none".to_string(),
                configured: false,
                provider_id: None,
                protocol: String::new(),
                base_url: None,
                has_key: false,
            },
        }
    }

    pub fn set_api_key(&self, key: &str) -> Result<ModelStatus, String> {
        self.require_writable()?;
        match self.services.model() {
            Some(model) => model.set_api_key(key),
            None => Err(i18n::t("Model gateway is not available")),
        }
    }

    pub fn run_status(&self, run_id: &str) -> Option<RunStatus> {
        if let Some(runtime) = self.services.runtime() {
            if let Some(status) = runtime.status(run_id) {
                return Some(status);
            }
        }
        // Completed runs leave the in-memory map; the journal projection is
        // the durable source (and the only source for another process).
        let session = self.services.session()?;
        let (session_id, run) = session.find_run(run_id)?;
        Some(RunStatus {
            run_id: run.id,
            session_id,
            state: run.state,
            steps: run.steps,
            max_steps: run.max_steps,
            cost_usd: run.cost_usd,
            max_cost_usd: run.max_cost_usd,
        })
    }

    pub fn save_provider(&self, input: ProviderInput) -> Result<ProviderView, String> {
        self.require_writable()?;
        self.providers.upsert(input)
    }

    pub fn remove_provider(&self, id: &str) -> Result<bool, String> {
        self.require_writable()?;
        self.providers.remove(id)
    }

    pub fn set_active_provider(&self, id: &str) -> Result<(), String> {
        self.require_writable()?;
        self.providers.set_active(id)
    }

    pub async fn test_provider(&self, id: Option<&str>) -> ProviderTestView {
        self.model_gateway.test_provider(id).await
    }

    pub async fn fetch_models(&self, id: Option<&str>) -> Result<Vec<String>, String> {
        let (_, models) = self.model_gateway.list_models(id).await?;
        Ok(models)
    }
}

pub fn budget_limits() -> (u64, f64, u64) {
    (
        env_u64("PHI_MAX_STEPS", 20),
        env_f64("PHI_MAX_COST_USD", 0.50),
        env_u64("PHI_MAX_WALL_SECS", 600),
    )
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}
