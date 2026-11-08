use std::sync::{Arc, RwLock};

use super::ports::{
    ApproverPort, ContextPort, ModelPort, PolicyPort, SandboxPort, SessionPort, ToolPort,
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
}
