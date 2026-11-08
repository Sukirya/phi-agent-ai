use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfig {
    pub node_id: Option<String>,
    pub workspace: Option<String>,
}

pub struct ConfigStore {
    path: PathBuf,
    inner: Mutex<AppConfig>,
}

impl ConfigStore {
    pub fn load(path: PathBuf) -> Self {
        let inner = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<AppConfig>(&raw).ok())
            .unwrap_or_default();
        Self {
            path,
            inner: Mutex::new(inner),
        }
    }

    pub fn snapshot(&self) -> AppConfig {
        self.inner.lock().unwrap().clone()
    }

    pub fn update<T>(&self, mutate: impl FnOnce(&mut AppConfig) -> T) -> T {
        let mut guard = self.inner.lock().unwrap();
        let result = mutate(&mut guard);
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(raw) = serde_json::to_string_pretty(&*guard) {
            let _ = fs::write(&self.path, raw);
        }
        result
    }
}
