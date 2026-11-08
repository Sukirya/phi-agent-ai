use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// A registration made by a plugin into a named extension point ("slot").
/// The kernel only tracks metadata; typed service handles live in `contracts`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Contribution {
    pub plugin_id: String,
    pub slot: String,
    pub id: String,
}

#[derive(Default)]
pub struct Registry {
    entries: Mutex<Vec<Contribution>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&self, contribution: Contribution) {
        self.entries.lock().unwrap().push(contribution);
    }

    pub fn remove_plugin(&self, plugin_id: &str) {
        self.entries
            .lock()
            .unwrap()
            .retain(|c| c.plugin_id != plugin_id);
    }

    pub fn has_slot(&self, slot: &str) -> bool {
        self.entries.lock().unwrap().iter().any(|c| c.slot == slot)
    }

    pub fn contributions(&self) -> Vec<Contribution> {
        self.entries.lock().unwrap().clone()
    }

    pub fn missing_slots(&self, required: &[&str]) -> Vec<String> {
        let entries = self.entries.lock().unwrap();
        required
            .iter()
            .copied()
            .filter(|slot| !entries.iter().any(|c| c.slot == *slot))
            .map(|slot| slot.to_string())
            .collect()
    }
}
