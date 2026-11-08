//! Kernel ports: mechanism-level interfaces only.
//!
//! This module must stay free of domain vocabulary (no Run/Session/Tool/etc.).
//! Domain ports live in `contracts`.

pub trait ClockPort: Send + Sync {
    /// Civil time used for display and audit.
    fn now_rfc3339(&self) -> String;
    /// Monotonic milliseconds used for durations and timeouts.
    fn monotonic_ms(&self) -> u64;
}

pub trait IdPort: Send + Sync {
    fn new_id(&self, prefix: &str) -> String;
}

/// Append-only event journal. Writes must be durable before returning.
pub trait JournalPort: Send + Sync {
    fn append(&self, line: &str) -> Result<(), String>;
    fn read_all(&self) -> Result<Vec<String>, String>;
}

/// Content-addressed blob storage for large payloads.
pub trait BlobPort: Send + Sync {
    fn put(&self, bytes: &[u8]) -> Result<String, String>;
    fn get(&self, content_ref: &str) -> Result<Vec<u8>, String>;
}

pub trait LogPort: Send + Sync {
    fn info(&self, msg: &str);
    fn warn(&self, msg: &str);
    fn error(&self, msg: &str);
}
