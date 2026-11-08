use crate::contracts::SLOT_PERSISTENCE_JOURNAL;
use crate::kernel::lifecycle::{Plugin, PluginContext};
use crate::kernel::ports::{BlobPort, JournalPort};
use crate::kernel::registry::Contribution;
use std::sync::Arc;

/// Baseline plugin that attests a real, durable journal is installed.
///
/// The kernel itself owns the mechanism port; this plugin makes the slot
/// explicit so required-slot evaluation (ADR-0009) is meaningful.
pub struct StorePlugin {
    journal: Arc<dyn JournalPort>,
    blob: Arc<dyn BlobPort>,
}

impl StorePlugin {
    pub fn new(journal: Arc<dyn JournalPort>, blob: Arc<dyn BlobPort>) -> Self {
        Self { journal, blob }
    }
}

impl Plugin for StorePlugin {
    fn id(&self) -> &'static str {
        "plugin-store"
    }

    fn activate(&self, _ctx: &PluginContext) -> Result<Vec<Contribution>, String> {
        // Prove both stores are usable without faking success.
        self.journal.read_all()?;
        let probe = self.blob.put(b"phi-store-probe")?;
        self.blob.get(&probe)?;
        Ok(vec![Contribution {
            plugin_id: self.id().to_string(),
            slot: SLOT_PERSISTENCE_JOURNAL.to_string(),
            id: "jsonl+fs-blob".to_string(),
        }])
    }
}
