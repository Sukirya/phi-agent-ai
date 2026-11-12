use std::sync::{Arc, Mutex, RwLock};

use serde_json::json;

use super::event_bus::EventBus;
use super::ports::LogPort;
use super::registry::{Contribution, Registry};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunMode {
    Normal,
    Degraded,
    Maintenance,
}

impl RunMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            RunMode::Normal => "Normal",
            RunMode::Degraded => "Degraded",
            RunMode::Maintenance => "Maintenance",
        }
    }
}

pub struct PluginContext {
    pub bus: Arc<EventBus>,
    pub registry: Arc<Registry>,
    pub log: Arc<dyn LogPort>,
}

/// A plugin is an in-process module that contributes to extension slots.
///
/// MVP note: plugins are compiled in and activated at startup. Runtime plugin
/// installation / the TS (QuickJS) ABI is a v1 item.
pub trait Plugin: Send + Sync {
    fn id(&self) -> &'static str;
    fn activate(&self, ctx: &PluginContext) -> Result<Vec<Contribution>, String>;
    fn deactivate(&self, _ctx: &PluginContext) -> Result<(), String> {
        Ok(())
    }
}

pub struct Kernel {
    pub bus: Arc<EventBus>,
    pub registry: Arc<Registry>,
    log: Arc<dyn LogPort>,
    plugins: Mutex<Vec<Arc<dyn Plugin>>>,
    active: Mutex<Vec<String>>,
    mode: RwLock<RunMode>,
    mode_reason: RwLock<String>,
}

impl Kernel {
    pub fn new(bus: Arc<EventBus>, registry: Arc<Registry>, log: Arc<dyn LogPort>) -> Self {
        Self {
            bus,
            registry,
            log,
            plugins: Mutex::new(Vec::new()),
            active: Mutex::new(Vec::new()),
            mode: RwLock::new(RunMode::Normal),
            mode_reason: RwLock::new(String::new()),
        }
    }

    pub fn add_plugin(&self, plugin: Arc<dyn Plugin>) {
        self.plugins.lock().unwrap().push(plugin);
    }

    pub fn activate_all(&self) {
        let plugins = self.plugins.lock().unwrap().clone();
        for plugin in plugins {
            let ctx = PluginContext {
                bus: self.bus.clone(),
                registry: self.registry.clone(),
                log: self.log.clone(),
            };
            match plugin.activate(&ctx) {
                Ok(contributions) => {
                    let slots: Vec<String> = contributions.iter().map(|c| c.slot.clone()).collect();
                    for c in contributions {
                        self.registry.add(c);
                    }
                    self.active.lock().unwrap().push(plugin.id().to_string());
                    let _ = self.bus.publish(
                        "node",
                        "kernel",
                        &json!({
                            "type": "PluginActivated",
                            "pluginId": plugin.id(),
                            "contributions": slots,
                        }),
                    );
                }
                Err(e) => {
                    self.log
                        .error(&format!("plugin {} failed to activate: {e}", plugin.id()));
                    let _ = self.bus.publish(
                        "node",
                        "kernel",
                        &json!({
                            "type": "PluginFailed",
                            "pluginId": plugin.id(),
                            "error": e,
                        }),
                    );
                }
            }
        }
    }

    pub fn deactivate_plugin(&self, plugin_id: &str) -> Result<(), String> {
        let plugin = {
            let plugins = self.plugins.lock().unwrap();
            plugins.iter().find(|p| p.id() == plugin_id).cloned()
        };
        let plugin = plugin.ok_or_else(|| format!("unknown plugin: {plugin_id}"))?;
        let ctx = PluginContext {
            bus: self.bus.clone(),
            registry: self.registry.clone(),
            log: self.log.clone(),
        };
        plugin.deactivate(&ctx)?;
        self.registry.remove_plugin(plugin_id);
        self.active.lock().unwrap().retain(|id| id != plugin_id);
        Ok(())
    }

    pub fn evaluate_mode(&self, required_slots: &[&str]) -> RunMode {
        let missing = self.registry.missing_slots(required_slots);
        if missing.is_empty() {
            RunMode::Normal
        } else {
            let reason = format!("required slots missing: {}", missing.join(", "));
            *self.mode_reason.write().unwrap() = reason;
            RunMode::Maintenance
        }
    }

    pub fn set_mode(&self, mode: RunMode, reason: &str) {
        *self.mode.write().unwrap() = mode;
        *self.mode_reason.write().unwrap() = reason.to_string();
        let _ = self.bus.publish(
            "node",
            "kernel",
            &json!({
                "type": "NodeModeChanged",
                "mode": mode.as_str(),
                "reason": reason,
            }),
        );
    }

    /// Set the mode during composition without publishing. Read-only shells do
    /// not have a writable journal to report the transition to.
    pub fn set_initial_mode(&self, mode: RunMode, reason: &str) {
        *self.mode.write().unwrap() = mode;
        *self.mode_reason.write().unwrap() = reason.to_string();
    }

    pub fn mode(&self) -> RunMode {
        *self.mode.read().unwrap()
    }

    pub fn mode_reason(&self) -> String {
        self.mode_reason.read().unwrap().clone()
    }

    pub fn active_plugins(&self) -> Vec<String> {
        self.active.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{ClockPort, IdPort, JournalPort, LogPort};
    use std::sync::Mutex;

    struct MemJournal(Mutex<Vec<String>>);
    impl JournalPort for MemJournal {
        fn append(&self, line: &str) -> Result<(), String> {
            self.0.lock().unwrap().push(line.to_string());
            Ok(())
        }
        fn read_all(&self) -> Result<Vec<String>, String> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    struct StubClock;
    impl ClockPort for StubClock {
        fn now_rfc3339(&self) -> String {
            "2026-01-01T00:00:00Z".to_string()
        }
        fn monotonic_ms(&self) -> u64 {
            0
        }
    }

    struct StubIds;
    impl IdPort for StubIds {
        fn new_id(&self, prefix: &str) -> String {
            format!("{prefix}_test")
        }
    }

    struct StubLog;
    impl LogPort for StubLog {
        fn info(&self, _msg: &str) {}
        fn warn(&self, _msg: &str) {}
        fn error(&self, _msg: &str) {}
    }

    struct FakePlugin {
        id: &'static str,
        slots: Vec<&'static str>,
    }

    impl Plugin for FakePlugin {
        fn id(&self) -> &'static str {
            self.id
        }
        fn activate(&self, _ctx: &PluginContext) -> Result<Vec<Contribution>, String> {
            Ok(self
                .slots
                .iter()
                .map(|slot| Contribution {
                    plugin_id: self.id.to_string(),
                    slot: slot.to_string(),
                    id: "test".to_string(),
                })
                .collect())
        }
    }

    fn kernel() -> Kernel {
        let bus = Arc::new(EventBus::new(
            Arc::new(MemJournal(Mutex::new(Vec::new()))),
            Arc::new(StubClock),
            Arc::new(StubIds),
            Arc::new(StubLog),
        ));
        Kernel::new(bus, Arc::new(Registry::new()), Arc::new(StubLog))
    }

    #[test]
    fn missing_required_slot_enters_maintenance() {
        let kernel = kernel();
        kernel.add_plugin(Arc::new(FakePlugin {
            id: "p1",
            slots: vec!["a", "b"],
        }));
        kernel.activate_all();
        assert_eq!(kernel.evaluate_mode(&["a", "b"]), RunMode::Normal);
        assert_eq!(
            kernel.evaluate_mode(&["a", "b", "missing"]),
            RunMode::Maintenance
        );
        assert!(kernel.mode_reason().contains("missing"));
    }

    #[test]
    fn deactivate_removes_contributions() {
        let kernel = kernel();
        kernel.add_plugin(Arc::new(FakePlugin {
            id: "p1",
            slots: vec!["a"],
        }));
        kernel.activate_all();
        assert!(kernel.registry.has_slot("a"));
        kernel.deactivate_plugin("p1").unwrap();
        assert!(!kernel.registry.has_slot("a"));
        assert_eq!(kernel.evaluate_mode(&["a"]), RunMode::Maintenance);
    }
}
