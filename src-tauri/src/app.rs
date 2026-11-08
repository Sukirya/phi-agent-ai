use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, Manager};

use crate::adapters::config::ConfigStore;
use crate::adapters::fs_blob::FsBlobStore;
use crate::adapters::fs_journal::JsonlJournal;
use crate::adapters::system::{StderrLog, SystemClock, UuidIds};
use crate::adapters::tauri_sink::TauriEventSink;
use crate::contracts::services::Services;
use crate::contracts::REQUIRED_SLOTS;
use crate::kernel::event_bus::EventBus;
use crate::kernel::lifecycle::{Kernel, RunMode};
use crate::kernel::ports::LogPort;
use crate::kernel::registry::Registry;
use crate::plugins::approver::HumanApprover;
use crate::plugins::context::DefaultContextPlugin;
use crate::plugins::loop_plugin::LoopPlugin;
use crate::plugins::model_gateway::ModelGateway;
use crate::plugins::policy::DefaultPolicyPlugin;
use crate::plugins::sandbox::FsSandboxPlugin;
use crate::plugins::session::SessionPlugin;
use crate::plugins::store::StorePlugin;
use crate::plugins::tools::BuiltinToolsPlugin;

pub struct AppState {
    pub node_id: String,
    pub bus: Arc<EventBus>,
    pub kernel: Arc<Kernel>,
    pub services: Arc<Services>,
    pub session: Arc<SessionPlugin>,
    pub runtime: Arc<LoopPlugin>,
    pub approver: Arc<HumanApprover>,
    pub config: Arc<ConfigStore>,
}

pub struct AppStateHandle(pub Arc<AppState>);

pub fn compose(app: AppHandle) -> Result<AppState, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("无法定位应用数据目录: {e}"))?;
    std::fs::create_dir_all(&data_dir).map_err(|e| format!("创建数据目录失败: {e}"))?;

    let journal = Arc::new(JsonlJournal::new(data_dir.join("journal").join("events.jsonl")));
    let blob = Arc::new(FsBlobStore::new(data_dir.join("blobs")));
    let clock = Arc::new(SystemClock::new());
    let ids = Arc::new(UuidIds);
    let log = Arc::new(StderrLog);
    let config = Arc::new(ConfigStore::load(data_dir.join("config.json")));

    let node_id = config.update(|config| {
        if config.node_id.is_none() {
            config.node_id = Some(format!("node_{}", uuid::Uuid::new_v4().simple()));
        }
        config.node_id.clone().unwrap_or_default()
    });

    // First-run default workspace, so the app is usable without configuration.
    let workspace = match config.snapshot().workspace {
        Some(path) if PathBuf::from(&path).is_dir() => PathBuf::from(path),
        _ => {
            let base = app.path().document_dir().unwrap_or_else(|_| data_dir.clone());
            let workspace = base.join("phi-workspace");
            std::fs::create_dir_all(&workspace)
                .map_err(|e| format!("创建默认工作区失败: {e}"))?;
            let display = workspace.to_string_lossy().to_string();
            config.update(|config| config.workspace = Some(display));
            workspace
        }
    };
    log.info(&format!("workspace: {}", workspace.display()));

    let bus = Arc::new(EventBus::new(
        journal.clone(),
        clock.clone(),
        ids.clone(),
        log.clone(),
    ));
    let records = bus.restore()?;

    let session = Arc::new(SessionPlugin::new(
        bus.clone(),
        ids.clone(),
        clock.clone(),
        blob.clone(),
        node_id.clone(),
    ));
    session.load(&records);

    // Projections must observe events before any live publishing happens.
    bus.subscribe(session.clone());
    bus.subscribe(Arc::new(TauriEventSink::new(app.clone())));

    let sandbox = Arc::new(FsSandboxPlugin);
    let model = Arc::new(ModelGateway::new(ids.clone()));
    let policy = Arc::new(DefaultPolicyPlugin::new(sandbox.clone()));
    let approver = Arc::new(HumanApprover::new(
        bus.clone(),
        ids.clone(),
        clock.clone(),
    ));
    let tools = Arc::new(BuiltinToolsPlugin::new(
        sandbox.clone(),
        data_dir.join("backups"),
    ));
    let context = Arc::new(DefaultContextPlugin::new(sandbox.clone()));

    let services = Arc::new(Services::new());
    services.set_session(session.clone());
    services.set_model(model.clone());
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

    let registry = Arc::new(Registry::new());
    let kernel = Arc::new(Kernel::new(
        bus.clone(),
        registry.clone(),
        log.clone(),
    ));
    kernel.add_plugin(Arc::new(StorePlugin::new(
        journal.clone(),
        blob.clone(),
    )));
    kernel.add_plugin(sandbox.clone());
    kernel.add_plugin(model.clone());
    kernel.add_plugin(policy.clone());
    kernel.add_plugin(approver.clone());
    kernel.add_plugin(session.clone());
    kernel.add_plugin(tools.clone());
    kernel.add_plugin(context.clone());
    kernel.add_plugin(runtime.clone());
    kernel.activate_all();

    let mode = kernel.evaluate_mode(REQUIRED_SLOTS);
    match mode {
        RunMode::Normal => kernel.set_mode(RunMode::Normal, "所有必需槽位齐备"),
        other => kernel.set_mode(
            other,
            &format!(
                "运行模式 {}：{}",
                other.as_str(),
                kernel.mode_reason()
            ),
        ),
    }

    let interrupted = session.recover_interrupted();
    if interrupted > 0 {
        log.warn(&format!(
            "{interrupted} interrupted run(s) marked AwaitingReconciliation"
        ));
    }

    Ok(AppState {
        node_id,
        bus,
        kernel,
        services,
        session,
        runtime,
        approver,
        config,
    })
}
