//! File-based control channel for shells that run in separate processes.
//!
//! `phi-headless approve` and `phi-headless cancel` cannot reach another
//! process's memory, so they drop a command file in the data directory. A
//! writable node watches the directory and applies the command in-process.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use phi_kernel::fs_atomic::write_atomic;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalCommand {
    pub approval_id: String,
    pub approved: bool,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelCommand {
    pub run_id: String,
}

pub fn request_approval(
    data_dir: &Path,
    approval_id: &str,
    approved: bool,
    note: Option<String>,
) -> Result<(), String> {
    let command = ApprovalCommand {
        approval_id: approval_id.to_string(),
        approved,
        note,
    };
    write_atomic(
        &approvals_dir(data_dir).join(format!("{}.json", safe_id(approval_id))),
        &serde_json::to_vec(&command).map_err(|error| error.to_string())?,
    )
}

pub fn request_cancel(data_dir: &Path, run_id: &str) -> Result<(), String> {
    let command = CancelCommand {
        run_id: run_id.to_string(),
    };
    write_atomic(
        &cancels_dir(data_dir).join(format!("{}.json", safe_id(run_id))),
        &serde_json::to_vec(&command).map_err(|error| error.to_string())?,
    )
}

pub fn take_approvals(data_dir: &Path) -> Vec<ApprovalCommand> {
    take_all(&approvals_dir(data_dir))
}

pub fn take_cancels(data_dir: &Path) -> Vec<CancelCommand> {
    take_all(&cancels_dir(data_dir))
}

/// Remove command files left behind by a shell whose target process was not
/// running. Otherwise a stale approval could be applied to a later process.
pub fn clean_stale(data_dir: &Path, max_age: Duration) {
    for dir in [approvals_dir(data_dir), cancels_dir(data_dir)] {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let now = SystemTime::now();
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let stale = metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .map(|age| age > max_age)
                .unwrap_or(false);
            if stale {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

fn take_all<T: for<'de> Deserialize<'de>>(dir: &Path) -> Vec<T> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = fs::read(&path) else {
            continue;
        };
        let _ = fs::remove_file(&path);
        if let Ok(command) = serde_json::from_slice::<T>(&raw) {
            out.push(command);
        }
    }
    out
}

fn approvals_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("control").join("approvals")
}

fn cancels_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("control").join("cancels")
}

fn safe_id(id: &str) -> String {
    let safe: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    if safe.is_empty() {
        "unknown".to_string()
    } else {
        safe
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("phi-control-{name}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn approvals_and_cancels_round_trip_once() {
        let dir = temp_dir("roundtrip");
        request_approval(&dir, "appr_1", true, Some("ok".to_string())).unwrap();
        request_cancel(&dir, "run_1").unwrap();

        let approvals = take_approvals(&dir);
        assert_eq!(approvals.len(), 1);
        assert_eq!(approvals[0].approval_id, "appr_1");
        assert!(approvals[0].approved);
        assert!(take_approvals(&dir).is_empty());

        let cancels = take_cancels(&dir);
        assert_eq!(cancels.len(), 1);
        assert_eq!(cancels[0].run_id, "run_1");
        assert!(take_cancels(&dir).is_empty());
    }
}
