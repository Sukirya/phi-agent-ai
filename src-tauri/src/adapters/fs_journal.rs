use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::kernel::ports::JournalPort;

/// Append-only JSONL journal. Every append is flushed and synced before the
/// call returns, so subscribers only ever observe durable events.
pub struct JsonlJournal {
    path: PathBuf,
    lock: Mutex<()>,
}

impl JsonlJournal {
    pub fn new(path: PathBuf) -> Self {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        Self {
            path,
            lock: Mutex::new(()),
        }
    }
}

impl JournalPort for JsonlJournal {
    fn append(&self, line: &str) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap();
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("open journal: {e}"))?;
        // Repair a torn tail first. A crash can leave the last record without
        // its terminating newline; appending directly would concatenate two
        // events into one unparseable line and lose both on replay.
        if file.metadata().map(|m| m.len()).unwrap_or(0) > 0 {
            file.seek(SeekFrom::End(-1))
                .map_err(|e| format!("seek journal: {e}"))?;
            let mut last = [0u8; 1];
            file.read_exact(&mut last)
                .map_err(|e| format!("read journal: {e}"))?;
            if last[0] != b'\n' {
                file.write_all(b"\n")
                    .map_err(|e| format!("repair journal tail: {e}"))?;
            }
        }
        file.write_all(line.as_bytes())
            .map_err(|e| format!("write journal: {e}"))?;
        file.write_all(b"\n")
            .map_err(|e| format!("write journal: {e}"))?;
        file.flush().map_err(|e| format!("flush journal: {e}"))?;
        file.sync_data().map_err(|e| format!("sync journal: {e}"))?;
        Ok(())
    }

    fn read_all(&self) -> Result<Vec<String>, String> {
        let _guard = self.lock.lock().unwrap();
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        // Read bytes and decode lossily: a tail truncated in the middle of a
        // UTF-8 sequence must not make the whole journal unreadable. The
        // resulting invalid line simply fails JSON parsing and is skipped.
        let raw = fs::read(&self.path).map_err(|e| format!("read journal: {e}"))?;
        let text = String::from_utf8_lossy(&raw);
        Ok(text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.to_string())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_journal(name: &str) -> (PathBuf, JsonlJournal) {
        let dir = std::env::temp_dir().join(format!(
            "phi-journal-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("events.jsonl");
        (path.clone(), JsonlJournal::new(path))
    }

    #[test]
    fn repairs_torn_tail_before_appending() {
        let (path, journal) = temp_journal("torn");
        // Simulate a crash between writing the JSON payload and its newline.
        fs::write(&path, b"{\"partial\":true}").unwrap();
        journal.append("{\"ok\":1}").unwrap();

        let lines = journal.read_all().unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "{\"partial\":true}");
        assert_eq!(lines[1], "{\"ok\":1}");
    }

    #[test]
    fn tolerates_truncated_utf8_tail() {
        let (path, journal) = temp_journal("utf8");
        let mut bytes = b"{\"a\":1}\n".to_vec();
        // A multi-byte character cut in half, with no terminating newline.
        bytes.extend_from_slice(&[0xE4, 0xB8]);
        fs::write(&path, &bytes).unwrap();

        let lines = journal.read_all().unwrap();
        assert_eq!(lines[0], "{\"a\":1}");
        assert_eq!(lines.len(), 2);
    }
}
