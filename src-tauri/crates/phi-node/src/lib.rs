//! Shell-independent node composition and application services.
//!
//! Desktop and headless shells both call [`compose`] and use the same [`Node`].
//! No Tauri type may appear here.

pub mod node;

pub use node::{Bootstrap, Node, NodeConfig, SendResult, SessionDetail, budget_limits, compose};
