//! Single-writer guard for a data directory.
//!
//! Journal sequence numbers, session projections and config files assume one
//! writer. Read-only shells do not lock: atomic file replacement and torn-line
//! tolerant journal reads keep them safe while a writer is active.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;

use phi_i18n as i18n;

pub struct DataDirLock {
    file: File,
}

impl DataDirLock {
    pub fn acquire(data_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(data_dir).map_err(|error| {
            i18n::t_args(
                "Failed to create data directory: {error}",
                &[("error", error.to_string())],
            )
        })?;
        let path = data_dir.join("phi.lock");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| {
                i18n::t_args(
                    "Failed to open data directory lock: {error}",
                    &[("error", error.to_string())],
                )
            })?;
        match file.try_lock() {
            Ok(()) => Ok(Self { file }),
            Err(TryLockError::WouldBlock) => Err(i18n::t(
                "Another phi-agent process is using this data directory",
            )),
            Err(TryLockError::Error(error)) => Err(i18n::t_args(
                "Failed to lock data directory: {error}",
                &[("error", error.to_string())],
            )),
        }
    }
}

impl Drop for DataDirLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}
