use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ports::{ClockPort, IdPort, JournalPort, LogPort};

/// A subscriber of the event stream. Invoked synchronously after the event has
/// been durably appended to the journal (persist first, notify second).
pub trait EventSink: Send + Sync {
    fn on_event(&self, record: &EventRecord);
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRecord {
    pub event_id: String,
    pub schema_version: u32,
    pub seq: u64,
    pub aggregate_id: String,
    pub aggregate_version: u64,
    pub ts: String,
    pub producer: String,
    pub payload: Value,
}

pub struct EventBus {
    journal: Arc<dyn JournalPort>,
    clock: Arc<dyn ClockPort>,
    ids: Arc<dyn IdPort>,
    log: Arc<dyn LogPort>,
    sinks: Mutex<Vec<Arc<dyn EventSink>>>,
    versions: Mutex<HashMap<String, u64>>,
    /// Serializes sequence allocation, journal append and sink notification so
    /// concurrent publishers cannot produce seq inversions or let live
    /// projections observe a different order than journal replay.
    publish_lock: Mutex<()>,
    read_only: AtomicBool,
    seq: AtomicU64,
}

impl EventBus {
    pub fn new(
        journal: Arc<dyn JournalPort>,
        clock: Arc<dyn ClockPort>,
        ids: Arc<dyn IdPort>,
        log: Arc<dyn LogPort>,
    ) -> Self {
        Self {
            journal,
            clock,
            ids,
            log,
            sinks: Mutex::new(Vec::new()),
            versions: Mutex::new(HashMap::new()),
            publish_lock: Mutex::new(()),
            read_only: AtomicBool::new(false),
            seq: AtomicU64::new(0),
        }
    }

    /// Turn the bus into a projector without a writer. Read-only shells still
    /// replay the journal and notify sinks, but any accidental publish fails
    /// instead of writing to a data directory owned by another process.
    pub fn set_read_only(&self) {
        self.read_only.store(true, Ordering::SeqCst);
    }

    pub fn subscribe(&self, sink: Arc<dyn EventSink>) {
        self.sinks.lock().unwrap().push(sink);
    }

    /// Append a domain event. The payload is serialized as-is so the kernel
    /// never needs to know the concrete event type.
    pub fn publish<T: Serialize>(
        &self,
        aggregate_id: &str,
        producer: &str,
        event: &T,
    ) -> Result<EventRecord, String> {
        let payload = serde_json::to_value(event).map_err(|e| e.to_string())?;
        self.append_raw(aggregate_id, producer, payload)
    }

    pub fn append_raw(
        &self,
        aggregate_id: &str,
        producer: &str,
        payload: Value,
    ) -> Result<EventRecord, String> {
        if self.read_only.load(Ordering::SeqCst) {
            return Err("event bus is read-only".to_string());
        }
        let _publish_guard = self.publish_lock.lock().unwrap();
        let (aggregate_version, seq) = {
            let mut versions = self.versions.lock().unwrap();
            let version = versions.entry(aggregate_id.to_string()).or_insert(0);
            *version += 1;
            let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
            (*version, seq)
        };

        let record = EventRecord {
            event_id: self.ids.new_id("evt"),
            schema_version: 1,
            seq,
            aggregate_id: aggregate_id.to_string(),
            aggregate_version,
            ts: self.clock.now_rfc3339(),
            producer: producer.to_string(),
            payload,
        };

        let line = serde_json::to_string(&record).map_err(|e| e.to_string())?;
        if let Err(e) = self.journal.append(&line) {
            self.log.error(&format!("journal append failed: {e}"));
            return Err(format!("journal append failed: {e}"));
        }

        let sinks = self.sinks.lock().unwrap().clone();
        for sink in sinks {
            sink.on_event(&record);
        }
        Ok(record)
    }

    /// Re-read the journal and seed seq/aggregate versions. Returns the parsed
    /// records in journal order. Must be called before subscribing projections.
    pub fn restore(&self) -> Result<Vec<EventRecord>, String> {
        let lines = self.journal.read_all()?;
        let mut records = Vec::with_capacity(lines.len());
        let mut versions: HashMap<String, u64> = HashMap::new();
        let mut max_seq = 0u64;

        for line in lines {
            match serde_json::from_str::<EventRecord>(&line) {
                Ok(record) => {
                    max_seq = max_seq.max(record.seq);
                    versions.insert(record.aggregate_id.clone(), record.aggregate_version);
                    records.push(record);
                }
                Err(e) => {
                    // A torn/corrupt tail must not kill the node; skip it loudly.
                    self.log
                        .warn(&format!("skipping unreadable journal line: {e}"));
                }
            }
        }

        *self.versions.lock().unwrap() = versions;
        self.seq.store(max_seq, Ordering::SeqCst);
        Ok(records)
    }
}
