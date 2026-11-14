//! Ports (interfaces), typed service handles, provider configuration and
//! extension-slot names.
//!
//! `phi-contracts` depends on `phi-domain`; plugin implementations depend on
//! both. Nothing here may reference a concrete plugin or a shell type.

pub mod ports;
pub mod provider;
pub mod services;
pub mod stream;
pub mod ui;
pub mod util;

// Extension point / required slot names.
pub const SLOT_RUNTIME_SKELETON: &str = "runtime.skeleton";
pub const SLOT_SESSION_STORE: &str = "session.store";
pub const SLOT_PERSISTENCE_JOURNAL: &str = "persistence.journal";
pub const SLOT_POLICY_EVALUATE: &str = "policy.evaluate";
pub const SLOT_SANDBOX_EXECUTE: &str = "sandbox.execute";
pub const SLOT_APPROVAL_DECIDE: &str = "approval.decide";
pub const SLOT_MODEL_INVOKE: &str = "model.invoke";
pub const SLOT_TOOL_PROVIDER: &str = "provider.tool";
pub const SLOT_CONTEXT_CONTRIBUTOR: &str = "context.contributor";

/// ADR-0009 required slots for the official distribution.
pub const REQUIRED_SLOTS: &[&str] = &[
    SLOT_RUNTIME_SKELETON,
    SLOT_SESSION_STORE,
    SLOT_PERSISTENCE_JOURNAL,
    SLOT_POLICY_EVALUATE,
    SLOT_SANDBOX_EXECUTE,
    SLOT_APPROVAL_DECIDE,
    SLOT_MODEL_INVOKE,
];
