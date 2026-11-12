//! Domain entities, events and read models.
//!
//! The domain crate depends on nothing but `serde`; ports and plugins depend on
//! it, never the other way around.

pub mod events;
pub mod message;
pub mod view;
