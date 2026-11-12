use std::sync::{Arc, RwLock};

use super::ports::{
    ApproverPort, ContextPort, ModelPort, PolicyPort, RuntimePort, SandboxPort, SessionPort,
    ToolPort,
};

/// Typed service handles, wired by the composition root.
///
/// Plugins look each other up here by *contract*, never by concrete plugin
/// type (module-map rule R2). Missing optional services return `None`; callers
/// must degrade explicitly.
#[derive(Default)]
pub struct Services {
    session: RwLock<Option<Arc<dyn SessionPort>>>,
    model: RwLock<Option<Arc<dyn ModelPort>>>,
    policy: RwLock<Option<Arc<dyn PolicyPort>>>,
    sandbox: RwLock<Option<Arc<dyn SandboxPort>>>,
    approver: RwLock<Option<Arc<dyn ApproverPort>>>,
    context: RwLock<Option<Arc<dyn ContextPort>>>,
    runtime: RwLock<Option<Arc<dyn RuntimePort>>>,
    tools: RwLock<Vec<Arc<dyn ToolPort>>>,
}

impl Services {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_session(&self, value: Arc<dyn SessionPort>) {
        *self.session.write().unwrap() = Some(value);
    }
    pub fn session(&self) -> Option<Arc<dyn SessionPort>> {
        self.session.read().unwrap().clone()
    }

    pub fn set_model(&self, value: Arc<dyn ModelPort>) {
        *self.model.write().unwrap() = Some(value);
    }
    pub fn model(&self) -> Option<Arc<dyn ModelPort>> {
        self.model.read().unwrap().clone()
    }

    pub fn set_policy(&self, value: Arc<dyn PolicyPort>) {
        *self.policy.write().unwrap() = Some(value);
    }
    pub fn policy(&self) -> Option<Arc<dyn PolicyPort>> {
        self.policy.read().unwrap().clone()
    }

    pub fn set_sandbox(&self, value: Arc<dyn SandboxPort>) {
        *self.sandbox.write().unwrap() = Some(value);
    }
    pub fn sandbox(&self) -> Option<Arc<dyn SandboxPort>> {
        self.sandbox.read().unwrap().clone()
    }

    pub fn set_approver(&self, value: Arc<dyn ApproverPort>) {
        *self.approver.write().unwrap() = Some(value);
    }
    pub fn approver(&self) -> Option<Arc<dyn ApproverPort>> {
        self.approver.read().unwrap().clone()
    }

    pub fn set_context(&self, value: Arc<dyn ContextPort>) {
        *self.context.write().unwrap() = Some(value);
    }
    pub fn context(&self) -> Option<Arc<dyn ContextPort>> {
        self.context.read().unwrap().clone()
    }

    pub fn add_tool_provider(&self, value: Arc<dyn ToolPort>) {
        self.tools.write().unwrap().push(value);
    }
    pub fn runtime(&self) -> Option<Arc<dyn RuntimePort>> {
        self.runtime.read().unwrap().clone()
    }
    pub fn set_runtime(&self, value: Arc<dyn RuntimePort>) {
        *self.runtime.write().unwrap() = Some(value);
    }
    pub fn tool_providers(&self) -> Vec<Arc<dyn ToolPort>> {
        self.tools.read().unwrap().clone()
    }
    pub fn tool_definitions(&self) -> Vec<super::ports::ToolDefinition> {
        self.tools
            .read()
            .unwrap()
            .iter()
            .flat_map(|t| t.definitions())
            .collect()
    }
    pub fn find_tool(&self, tool_id: &str) -> Option<Arc<dyn ToolPort>> {
        self.tools
            .read()
            .unwrap()
            .iter()
            .find(|t| t.supports(tool_id))
            .cloned()
    }

    /// Check required slots against the typed wiring rather than trusting slot
    /// metadata alone. `persistence.journal` is owned by the kernel registry
    /// because the journal is a mechanism port, so it always counts as present
    /// here.
    pub fn missing_slots(&self, required: &[&str]) -> Vec<String> {
        let mut missing = Vec::new();
        for slot in required {
            let present = match *slot {
                crate::SLOT_RUNTIME_SKELETON => self.runtime().is_some(),
                crate::SLOT_SESSION_STORE => self.session().is_some(),
                crate::SLOT_POLICY_EVALUATE => self.policy().is_some(),
                crate::SLOT_SANDBOX_EXECUTE => self.sandbox().is_some(),
                crate::SLOT_APPROVAL_DECIDE => self.approver().is_some(),
                crate::SLOT_MODEL_INVOKE => self.model().is_some(),
                crate::SLOT_PERSISTENCE_JOURNAL => true,
                _ => false,
            };
            if !present {
                missing.push((*slot).to_string());
            }
        }
        missing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_services_report_missing_required_slots() {
        let services = Services::new();
        let missing = services.missing_slots(crate::REQUIRED_SLOTS);
        assert!(missing.contains(&crate::SLOT_RUNTIME_SKELETON.to_string()));
        assert!(missing.contains(&crate::SLOT_MODEL_INVOKE.to_string()));
        assert!(!missing.contains(&crate::SLOT_PERSISTENCE_JOURNAL.to_string()));
    }
}
