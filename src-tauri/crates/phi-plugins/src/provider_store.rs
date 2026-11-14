//! Durable provider profiles.
//!
//! The file lives next to the journal (`data_dir/providers.json`). It holds
//! endpoints, model mappings and *references* to credentials. List/read APIs
//! only ever expose a redacted hint; resolving a secret happens inside the
//! model gateway at call time.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use chrono::Utc;
use uuid::Uuid;

use phi_contracts::provider::{
    AuthMode, Protocol, ProviderFile, ProviderInput, ProviderProfile, ProviderView, SecretRef,
};

use phi_i18n as i18n;
use phi_kernel::fs_atomic::write_atomic;

#[derive(Clone, Debug)]
pub struct ResolvedProvider {
    pub profile: ProviderProfile,
    pub api_key: Option<String>,
}

pub struct ProviderStore {
    path: PathBuf,
    inner: RwLock<ProviderFile>,
}

impl ProviderStore {
    pub fn load(path: PathBuf) -> Self {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let backup = backup_path(&path);
        // A corrupt primary file must fall back to the last good backup
        // instead of silently resetting the user's providers.
        let mut file = read_provider_file(&path)
            .or_else(|| read_provider_file(&backup))
            .unwrap_or_default();
        if file.version == 0 {
            file.version = 1;
        }
        // Repair a dangling active id instead of carrying it forward.
        if let Some(active) = file.active_id.clone() {
            if !file.providers.iter().any(|provider| provider.id == active) {
                file.active_id = file
                    .providers
                    .iter()
                    .find(|provider| provider.enabled)
                    .map(|provider| provider.id.clone());
            }
        }
        if file.active_id.is_none() {
            file.active_id = file
                .providers
                .iter()
                .find(|provider| provider.enabled)
                .map(|provider| provider.id.clone());
        }
        Self {
            path,
            inner: RwLock::new(file),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn snapshot(&self) -> ProviderFile {
        self.inner.read().unwrap().clone()
    }

    pub fn views(&self) -> Vec<ProviderView> {
        let file = self.snapshot();
        let active = file.active_id.clone();
        let mut views: Vec<ProviderView> = file
            .providers
            .iter()
            .map(|profile| view_of(profile, active.as_deref() == Some(profile.id.as_str())))
            .collect();
        views.sort_by(|a, b| {
            a.sort_index
                .cmp(&b.sort_index)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        views
    }

    pub fn view(&self, id: &str) -> Option<ProviderView> {
        let file = self.snapshot();
        file.providers
            .iter()
            .find(|provider| provider.id == id)
            .map(|profile| view_of(profile, file.active_id.as_deref() == Some(id)))
    }

    pub fn active_id(&self) -> Option<String> {
        self.inner.read().unwrap().active_id.clone()
    }

    pub fn active(&self) -> Option<ProviderProfile> {
        let file = self.inner.read().unwrap();
        let id = file.active_id.as_deref()?;
        file.providers
            .iter()
            .find(|provider| provider.id == id)
            .cloned()
    }

    pub fn set_active(&self, id: &str) -> Result<(), String> {
        let mut file = self.inner.write().unwrap();
        let profile = file
            .providers
            .iter()
            .find(|provider| provider.id == id)
            .ok_or_else(|| i18n::t_args("Unknown provider: {id}", &[("id", id.to_string())]))?;
        if !profile.enabled {
            return Err(i18n::t_args(
                "Provider {name} is disabled",
                &[("name", profile.name.clone())],
            ));
        }
        file.active_id = Some(id.to_string());
        self.save(&file)?;
        Ok(())
    }

    /// Create or update a provider. `input.api_key == None` keeps the current
    /// credential; `Some("")` clears it.
    pub fn upsert(&self, input: ProviderInput) -> Result<ProviderView, String> {
        let mut file = self.inner.write().unwrap();
        let existing_index = match input.id.as_deref() {
            Some(id) => {
                let index = file
                    .providers
                    .iter()
                    .position(|provider| provider.id == id)
                    .ok_or_else(|| {
                        i18n::t_args("Unknown provider: {id}", &[("id", id.to_string())])
                    })?;
                Some(index)
            }
            None => None,
        };

        let now = Utc::now().to_rfc3339();
        let mut profile = match existing_index {
            Some(index) => file.providers[index].clone(),
            None => {
                let sort_index = file
                    .providers
                    .iter()
                    .map(|provider| provider.sort_index + 1)
                    .max()
                    .unwrap_or(0);
                ProviderProfile {
                    id: format!("prov_{}", Uuid::new_v4().simple()),
                    name: String::new(),
                    protocol: Protocol::AnthropicMessages,
                    base_url: String::new(),
                    api_key: None,
                    model: String::new(),
                    models: Vec::new(),
                    model_map: HashMap::new(),
                    auth: None,
                    headers: HashMap::new(),
                    pricing: None,
                    max_output_tokens: None,
                    temperature: None,
                    enabled: true,
                    sort_index,
                    icon: None,
                    icon_color: None,
                    website_url: None,
                    notes: None,
                    created_at: now.clone(),
                    updated_at: now.clone(),
                }
            }
        };

        let is_new = existing_index.is_none();
        if let Some(protocol) = input.protocol.as_deref() {
            profile.protocol = Protocol::parse(protocol)?;
            // Let the new protocol choose its default auth unless the caller
            // explicitly sets one below.
            if input.auth.is_none() {
                profile.auth = None;
            }
        }
        if let Some(name) = input.name {
            profile.name = name.trim().to_string();
        }
        if let Some(base_url) = input.base_url {
            profile.base_url = base_url.trim().trim_end_matches('/').to_string();
        }
        if let Some(model) = input.model {
            profile.model = model.trim().to_string();
        }
        if let Some(models) = input.models {
            profile.models = dedupe(models);
        }
        if let Some(model_map) = input.model_map {
            profile.model_map = model_map;
        }
        if let Some(auth) = input.auth.as_deref() {
            profile.auth = Some(AuthMode::parse(auth)?);
        }
        if let Some(api_key) = input.api_key.as_deref() {
            profile.api_key = SecretRef::normalize(api_key);
            if profile.api_key.is_none() && input.auth.is_none() {
                // Clearing the credential also drops the requirement for one;
                // an explicit auth mode in the same input still wins below.
                profile.auth = Some(AuthMode::None);
            }
        }
        if let Some(headers) = input.headers {
            validate_headers(&headers)?;
            profile.headers = headers;
        }
        if let Some(pricing) = input.pricing {
            if pricing.input_per_mtok < 0.0 || pricing.output_per_mtok < 0.0 {
                return Err(i18n::t("Pricing cannot be negative"));
            }
            profile.pricing = Some(pricing);
        }
        if let Some(max_output_tokens) = input.max_output_tokens {
            if max_output_tokens == 0 {
                return Err(i18n::t("maxOutputTokens must be greater than 0"));
            }
            profile.max_output_tokens = Some(max_output_tokens);
        }
        if let Some(temperature) = input.temperature {
            if !(0.0..=2.0).contains(&temperature) {
                return Err(i18n::t("temperature must be between 0 and 2"));
            }
            profile.temperature = Some(temperature);
        }
        if let Some(enabled) = input.enabled {
            profile.enabled = enabled;
        }
        if let Some(sort_index) = input.sort_index {
            profile.sort_index = sort_index;
        }
        if input.icon.is_some() {
            profile.icon = input.icon;
        }
        if input.icon_color.is_some() {
            profile.icon_color = input.icon_color;
        }
        if input.website_url.is_some() {
            profile.website_url = input.website_url;
        }
        if input.notes.is_some() {
            profile.notes = input.notes;
        }

        if profile.name.is_empty() {
            return Err(i18n::t("Provider name cannot be empty"));
        }
        if profile.base_url.is_empty() {
            return Err(i18n::t("baseUrl cannot be empty"));
        }
        if !(profile.base_url.starts_with("http://") || profile.base_url.starts_with("https://")) {
            return Err(i18n::t("baseUrl must start with http:// or https://"));
        }
        if profile.model.is_empty() {
            return Err(i18n::t("model cannot be empty"));
        }
        if profile.auth_mode() != AuthMode::None && profile.api_key.is_none() {
            return Err(i18n::t(
                "This auth mode requires an API key (an env:NAME reference works)",
            ));
        }

        profile.updated_at = now;
        match existing_index {
            Some(index) => file.providers[index] = profile.clone(),
            None => file.providers.push(profile.clone()),
        }
        if is_new && file.active_id.is_none() && profile.enabled {
            file.active_id = Some(profile.id.clone());
        }
        if file.active_id.is_none() && profile.enabled {
            file.active_id = Some(profile.id.clone());
        }
        self.save(&file)?;
        let active = file.active_id.as_deref() == Some(profile.id.as_str());
        Ok(view_of(&profile, active))
    }

    pub fn remove(&self, id: &str) -> Result<bool, String> {
        let mut file = self.inner.write().unwrap();
        let before = file.providers.len();
        file.providers.retain(|provider| provider.id != id);
        if file.providers.len() == before {
            return Ok(false);
        }
        if file.active_id.as_deref() == Some(id) {
            file.active_id = file
                .providers
                .iter()
                .find(|provider| provider.enabled)
                .map(|provider| provider.id.clone());
        }
        self.save(&file)?;
        Ok(true)
    }

    /// Set (or clear) the credential of the active provider. If no provider
    /// exists, a default Anthropic profile is created from the environment.
    pub fn set_api_key(&self, key: &str) -> Result<ProviderView, String> {
        let active = self.active_id();
        if let Some(id) = active {
            return self.upsert(ProviderInput {
                id: Some(id),
                api_key: Some(key.to_string()),
                ..Default::default()
            });
        }
        if key.trim().is_empty() {
            return Err(i18n::t("No provider is available"));
        }
        let base_url = std::env::var("PHI_ANTHROPIC_BASE_URL")
            .unwrap_or_else(|_| "https://api.anthropic.com".to_string());
        let model = std::env::var("PHI_MODEL").unwrap_or_else(|_| "claude-sonnet-5".to_string());
        self.upsert(ProviderInput {
            name: Some("Anthropic".to_string()),
            protocol: Some("anthropic_messages".to_string()),
            base_url: Some(base_url),
            api_key: Some(key.to_string()),
            model: Some(model),
            auth: Some("x_api_key".to_string()),
            ..Default::default()
        })
    }

    /// First-run convenience: mirror the legacy environment variables into a
    /// provider profile. Only runs when no profile exists and a key is set,
    /// so the offline mock path stays the default.
    pub fn seed_from_env(&self) -> Result<(), String> {
        {
            let file = self.inner.read().unwrap();
            if !file.providers.is_empty() {
                return Ok(());
            }
        }
        let (key_env, key_value) = match std::env::var("ANTHROPIC_API_KEY") {
            Ok(value) if !value.trim().is_empty() => ("ANTHROPIC_API_KEY", value),
            _ => match std::env::var("PHI_API_KEY") {
                Ok(value) if !value.trim().is_empty() => ("PHI_API_KEY", value),
                _ => return Ok(()),
            },
        };
        let _ = key_value;
        let base_url = std::env::var("PHI_ANTHROPIC_BASE_URL")
            .unwrap_or_else(|_| "https://api.anthropic.com".to_string());
        let model = std::env::var("PHI_MODEL").unwrap_or_else(|_| "claude-sonnet-5".to_string());
        self.upsert(ProviderInput {
            name: Some("Anthropic".to_string()),
            protocol: Some("anthropic_messages".to_string()),
            base_url: Some(base_url),
            api_key: Some(format!("env:{key_env}")),
            model: Some(model),
            auth: Some("x_api_key".to_string()),
            ..Default::default()
        })?;
        Ok(())
    }

    pub fn mark_models(&self, id: &str, models: Vec<String>) -> Result<(), String> {
        let mut file = self.inner.write().unwrap();
        let profile = file
            .providers
            .iter_mut()
            .find(|provider| provider.id == id)
            .ok_or_else(|| i18n::t_args("Unknown provider: {id}", &[("id", id.to_string())]))?;
        profile.models = dedupe(models);
        if profile.model.is_empty() {
            if let Some(first) = profile.models.first() {
                profile.model = first.clone();
            }
        }
        profile.updated_at = Utc::now().to_rfc3339();
        self.save(&file)?;
        Ok(())
    }

    pub fn resolve_active(&self) -> Result<Option<ResolvedProvider>, String> {
        match self.active() {
            Some(profile) => self.resolve_profile(&profile).map(Some),
            None => Ok(None),
        }
    }

    pub fn resolve(&self, id: &str) -> Result<ResolvedProvider, String> {
        let file = self.snapshot();
        let profile = file
            .providers
            .iter()
            .find(|provider| provider.id == id)
            .cloned()
            .ok_or_else(|| i18n::t_args("Unknown provider: {id}", &[("id", id.to_string())]))?;
        self.resolve_profile(&profile)
    }

    fn resolve_profile(&self, profile: &ProviderProfile) -> Result<ResolvedProvider, String> {
        if !profile.enabled {
            return Err(i18n::t_args(
                "Provider {name} is disabled",
                &[("name", profile.name.clone())],
            ));
        }
        if profile.base_url.trim().is_empty() {
            return Err(i18n::t_args(
                "Provider {name} is missing baseUrl",
                &[("name", profile.name.clone())],
            ));
        }
        if profile.model.trim().is_empty() {
            return Err(i18n::t_args(
                "Provider {name} is missing model",
                &[("name", profile.name.clone())],
            ));
        }
        let api_key = match profile.auth_mode() {
            AuthMode::None => None,
            _ => match &profile.api_key {
                Some(secret) => Some(secret.resolve().map_err(|error| {
                    i18n::t_args(
                        "Provider {name}: {error}",
                        &[("name", profile.name.clone()), ("error", error)],
                    )
                })?),
                None => {
                    return Err(i18n::t_args(
                        "Provider {name} has no API key",
                        &[("name", profile.name.clone())],
                    ));
                }
            },
        };
        Ok(ResolvedProvider {
            profile: profile.clone(),
            api_key,
        })
    }

    fn save(&self, file: &ProviderFile) -> Result<(), String> {
        let raw = serde_json::to_string_pretty(file).map_err(|error| error.to_string())?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                i18n::t_args(
                    "Failed to create config directory: {error}",
                    &[("error", error.to_string())],
                )
            })?;
        }
        let backup = backup_path(&self.path);
        if self.path.exists() {
            let _ = fs::copy(&self.path, &backup);
        }
        write_atomic(&self.path, raw.as_bytes()).map_err(|error| {
            i18n::t_args(
                "Failed to replace provider config: {error}",
                &[("error", error)],
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
}

fn view_of(profile: &ProviderProfile, active: bool) -> ProviderView {
    ProviderView {
        id: profile.id.clone(),
        name: profile.name.clone(),
        protocol: profile.protocol.as_str().to_string(),
        base_url: profile.base_url.clone(),
        model: profile.model.clone(),
        models: profile.models.clone(),
        enabled: profile.enabled,
        active,
        has_key: profile.has_key(),
        key_hint: profile.api_key.as_ref().map(SecretRef::hint),
        auth: profile.auth_mode().as_str().to_string(),
        pricing: profile.pricing_or_default(),
        max_output_tokens: profile.max_output_tokens,
        temperature: profile.temperature,
        sort_index: profile.sort_index,
        icon: profile.icon.clone(),
        icon_color: profile.icon_color.clone(),
        website_url: profile.website_url.clone(),
        notes: profile.notes.clone(),
        created_at: profile.created_at.clone(),
        updated_at: profile.updated_at.clone(),
    }
}

fn dedupe(items: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let item = item.trim().to_string();
        if !item.is_empty() && !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

fn validate_headers(headers: &HashMap<String, String>) -> Result<(), String> {
    for (key, value) in headers {
        let name = key.trim().to_ascii_lowercase();
        if name.is_empty() {
            return Err(i18n::t("Custom header name cannot be empty"));
        }
        if name == "authorization" || name == "x-api-key" || name == "anthropic-version" {
            return Err(i18n::t_args(
                "Header {key} is managed by the gateway and cannot be overridden in custom headers",
                &[("key", key.clone())],
            ));
        }
        if key.contains('\r') || key.contains('\n') || value.contains('\r') || value.contains('\n')
        {
            return Err(i18n::t_args(
                "Header {key} contains an invalid newline",
                &[("key", key.clone())],
            ));
        }
    }
    Ok(())
}

fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".bak");
    path.with_file_name(name)
}

fn read_provider_file(path: &Path) -> Option<ProviderFile> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str::<ProviderFile>(&raw).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> ProviderStore {
        let dir =
            std::env::temp_dir().join(format!("phi-providers-{name}-{}", uuid::Uuid::new_v4()));
        ProviderStore::load(dir.join("providers.json"))
    }

    fn input(name: &str) -> ProviderInput {
        ProviderInput {
            name: Some(name.to_string()),
            protocol: Some("openai_chat".to_string()),
            base_url: Some("https://example.com".to_string()),
            api_key: Some("env:PHI_TEST_KEY".to_string()),
            model: Some("test-model".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn add_activate_and_persist() {
        let store = store("persist");
        let view = store.upsert(input("One")).unwrap();
        assert!(view.active);
        assert_eq!(view.protocol, "openai_chat");
        assert_eq!(view.key_hint.as_deref(), Some("env:PHI_TEST_KEY"));
        assert!(!view.key_hint.as_deref().unwrap().contains("sk-"));

        let reloaded = ProviderStore::load(store.path().to_path_buf());
        assert_eq!(reloaded.views().len(), 1);
        assert_eq!(reloaded.active_id(), Some(view.id));
    }

    #[test]
    fn remove_active_moves_selection() {
        let store = store("remove");
        let first = store.upsert(input("One")).unwrap();
        let second = store.upsert(input("Two")).unwrap();
        assert!(first.active);
        assert!(store.remove(&first.id).unwrap());
        assert_eq!(store.active_id(), Some(second.id));
        assert!(!store.remove("missing").unwrap());
    }

    #[test]
    fn update_keeps_secret_when_omitted_and_clears_on_empty() {
        let store = store("secret");
        let view = store.upsert(input("One")).unwrap();
        let updated = store
            .upsert(ProviderInput {
                id: Some(view.id.clone()),
                name: Some("Renamed".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(updated.name, "Renamed");
        assert_eq!(updated.key_hint.as_deref(), Some("env:PHI_TEST_KEY"));

        let cleared = store
            .upsert(ProviderInput {
                id: Some(view.id),
                api_key: Some(String::new()),
                ..Default::default()
            })
            .unwrap();
        assert!(cleared.key_hint.is_none());
    }

    #[test]
    fn rejects_auth_headers_in_custom_headers() {
        let store = store("headers");
        let mut input = input("Bad");
        input.headers = Some(HashMap::from([(
            "Authorization".to_string(),
            "Bearer x".to_string(),
        )]));
        assert!(store.upsert(input).is_err());
    }

    #[test]
    fn literal_secret_resolves_and_is_redacted() {
        let store = store("literal");
        let mut input = input("Literal");
        input.api_key = Some("sk-real-secret".to_string());
        let view = store.upsert(input).unwrap();
        assert_eq!(view.key_hint.as_deref(), Some("literal:***"));
        let resolved = store.resolve(&view.id).unwrap();
        assert_eq!(resolved.api_key.as_deref(), Some("sk-real-secret"));
    }

    #[test]
    fn corrupt_primary_falls_back_to_the_backup() {
        let store = store("backup");
        let view = store.upsert(input("One")).unwrap();
        let path = store.path().to_path_buf();
        fs::copy(&path, backup_path(&path)).unwrap();
        fs::write(&path, b"{not json").unwrap();

        let reloaded = ProviderStore::load(path);
        assert_eq!(reloaded.views().len(), 1);
        assert_eq!(reloaded.active_id(), Some(view.id));
    }
}
