use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use phi_i18n as i18n;
use serde::{Deserialize, Serialize};

use phi_kernel::fs_atomic::write_atomic;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfig {
    pub node_id: Option<String>,
    pub workspace: Option<String>,
    /// BCP 47 tag such as `en` or `zh-CN`. Missing means "detect".
    pub language: Option<String>,
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

    /// Mutate and persist. Returns an error when the new state cannot be made
    /// durable, so callers never report success for a lost setting.
    pub fn update<T>(&self, mutate: impl FnOnce(&mut AppConfig) -> T) -> Result<T, String> {
        let mut guard = self.inner.lock().unwrap();
        let result = mutate(&mut guard);
        let raw = serde_json::to_string_pretty(&*guard).map_err(|error| {
            i18n::t_args(
                "Failed to serialize settings: {error}",
                &[("error", error.to_string())],
            )
        })?;
        write_atomic(&self.path, raw.as_bytes()).map_err(|error| {
            i18n::t_args("Failed to save settings: {error}", &[("error", error)])
        })?;
        Ok(result)
    }
}
