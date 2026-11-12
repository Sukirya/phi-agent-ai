//! Kernel mechanisms: event bus, plugin lifecycle, registry and mechanism ports.
//!
//! This crate must stay free of domain vocabulary (no Run/Session/Tool/etc.).

pub mod event_bus;
pub mod fs_atomic;
pub mod lifecycle;
pub mod ports;
pub mod registry;
