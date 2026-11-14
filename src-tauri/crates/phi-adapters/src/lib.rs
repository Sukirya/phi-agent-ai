//! Platform adapters for kernel ports and shell-facing sinks.
//!
//! Adapters implement ports; they never depend on plugins. The Tauri sinks live
//! in the desktop shell crate so no Tauri type appears here.

pub mod config;
pub mod control;
pub mod fs_blob;
pub mod fs_journal;
pub mod jsonl_sink;
pub mod lock;
pub mod system;
