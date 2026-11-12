//! Provider configuration shared by every shell.
//!
//! A provider profile describes *where* to send a model call and *how* to
//! speak to it. The wire protocol is explicit because a gateway may expose
//! Anthropic Messages, OpenAI Chat Completions and OpenAI Responses on the
//! same host; the client must never guess.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use phi_i18n as i18n;

/// Wire protocol used by a provider endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    AnthropicMessages,
    OpenAiChat,
    OpenAiResponses,
}

impl Default for Protocol {
    fn default() -> Self {
        Protocol::AnthropicMessages
    }
}

impl Protocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Protocol::AnthropicMessages => "anthropic_messages",
            Protocol::OpenAiChat => "openai_chat",
            Protocol::OpenAiResponses => "openai_responses",
        }
    }

    /// Accepts the canonical names plus the aliases users are likely to type.
    pub fn parse(value: &str) -> Result<Self, String> {
        let normalized = value.trim().to_ascii_lowercase().replace('-', "_");
        match normalized.as_str() {
            "anthropic" | "anthropic_messages" | "messages" | "claude" => {
                Ok(Protocol::AnthropicMessages)
            }
            "openai" | "openai_chat" | "chat" | "chat_completions" | "completions" => {
                Ok(Protocol::OpenAiChat)
            }
            "openai_responses" | "responses" | "response" => Ok(Protocol::OpenAiResponses),
            other => Err(i18n::t_args(
                "Unknown protocol {protocol}; expected anthropic_messages, openai_chat or openai_responses",
                &[("protocol", other.to_string())],
            )),
        }
    }

    /// Endpoint suffix appended to the provider's base URL.
    pub fn endpoint_suffix(&self) -> &'static str {
        match self {
            Protocol::AnthropicMessages => "/messages",
            Protocol::OpenAiChat => "/chat/completions",
            Protocol::OpenAiResponses => "/responses",
        }
    }

    pub fn default_auth(&self) -> AuthMode {
        match self {
            Protocol::AnthropicMessages => AuthMode::XApiKey,
            Protocol::OpenAiChat | Protocol::OpenAiResponses => AuthMode::Bearer,
        }
    }

    /// Conservative list prices used when a profile does not declare its own.
    /// They exist so the budget ledger has an upper bound; users can override
    /// them per provider (pricing changes should not silently rewrite history).
    pub fn default_pricing(&self) -> Pricing {
        match self {
            Protocol::AnthropicMessages => Pricing {
                input_per_mtok: 3.0,
                output_per_mtok: 15.0,
            },
            Protocol::OpenAiChat | Protocol::OpenAiResponses => Pricing {
                input_per_mtok: 2.5,
                output_per_mtok: 10.0,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    XApiKey,
    Bearer,
    None,
}

impl Default for AuthMode {
    fn default() -> Self {
        AuthMode::None
    }
}

impl AuthMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthMode::XApiKey => "x_api_key",
            AuthMode::Bearer => "bearer",
            AuthMode::None => "none",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "x_api_key" | "apikey" | "anthropic" => Ok(AuthMode::XApiKey),
            "bearer" | "authorization" => Ok(AuthMode::Bearer),
            "none" | "no_auth" => Ok(AuthMode::None),
            other => Err(i18n::t_args(
                "Unknown auth mode {auth}; expected x_api_key, bearer or none",
                &[("auth", other.to_string())],
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pricing {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
}

/// A reference to a credential, never the credential itself when rendered.
///
/// Serialized as a plain string so the settings UI can edit it directly:
/// - `env:ANTHROPIC_API_KEY`
/// - `keychain:provider/openai`
/// - `literal:sk-...` (development only; still redacted in every read API)
///
/// A bare string without a prefix is treated as `literal:` for backward
/// compatibility with hand-written configs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretRef(pub String);

impl SecretRef {
    pub fn env(name: impl Into<String>) -> Self {
        Self(format!("env:{}", name.into()))
    }

    pub fn literal(value: impl Into<String>) -> Self {
        Self(format!("literal:{}", value.into()))
    }

    /// Empty input clears the secret. A bare value is stored as a literal.
    pub fn normalize(input: &str) -> Option<Self> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Some(value) = trimmed.strip_prefix("literal:") {
            if value.trim().is_empty() {
                return None;
            }
        }
        if trimmed.starts_with("env:")
            || trimmed.starts_with("keychain:")
            || trimmed.starts_with("literal:")
        {
            Some(Self(trimmed.to_string()))
        } else {
            Some(Self::literal(trimmed))
        }
    }

    pub fn kind(&self) -> &'static str {
        if self.0.starts_with("env:") {
            "env"
        } else if self.0.starts_with("keychain:") {
            "keychain"
        } else {
            "literal"
        }
    }

    pub fn resolve(&self) -> Result<String, String> {
        if let Some(name) = self.0.strip_prefix("env:") {
            let value = std::env::var(name).map_err(|_| {
                i18n::t_args(
                    "Environment variable {name} is not set",
                    &[("name", name.to_string())],
                )
            })?;
            if value.trim().is_empty() {
                return Err(i18n::t_args(
                    "Environment variable {name} is empty",
                    &[("name", name.to_string())],
                ));
            }
            Ok(value)
        } else if let Some(name) = self.0.strip_prefix("keychain:") {
            // The keychain adapter is a follow-up; fail honestly instead of
            // pretending a credential exists.
            Err(i18n::t_args(
                "OS Keychain is not enabled yet ({name}); use an env: reference or literal: for development",
                &[("name", name.to_string())],
            ))
        } else if let Some(value) = self.0.strip_prefix("literal:") {
            if value.trim().is_empty() {
                Err(i18n::t("API key is empty"))
            } else {
                Ok(value.to_string())
            }
        } else if self.0.trim().is_empty() {
            Err(i18n::t("API key is empty"))
        } else {
            Ok(self.0.clone())
        }
    }

    pub fn is_configured(&self) -> bool {
        self.resolve().is_ok()
    }

    /// Safe to return from list/read APIs.
    pub fn hint(&self) -> String {
        match self.kind() {
            "env" => self.0.clone(),
            "keychain" => self.0.clone(),
            _ => "literal:***".to_string(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderProfile {
    pub id: String,
    pub name: String,
    pub protocol: Protocol,
    #[serde(default)]
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<SecretRef>,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub models: Vec<String>,
    /// Maps a logical role ("default", "fast", a UI label) to a wire model id.
    #[serde(default)]
    pub model_map: HashMap<String, String>,
    /// `None` means "use the protocol's default"; `Some(AuthMode::None)`
    /// explicitly disables auth for local gateways.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthMode>,
    /// Extra headers. Auth headers are not allowed here; they come from
    /// `auth` + `api_key` so they can be redacted consistently.
    #[serde(default)]
    pub headers: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<Pricing>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub sort_index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub website_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

fn default_true() -> bool {
    true
}

/// Provider settings read model. Never contains a credential value.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    pub id: String,
    pub name: String,
    pub protocol: String,
    pub base_url: String,
    pub model: String,
    pub models: Vec<String>,
    pub enabled: bool,
    pub active: bool,
    pub has_key: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_hint: Option<String>,
    pub auth: String,
    pub pricing: Pricing,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    pub sort_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub website_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTestView {
    pub ok: bool,
    pub latency_ms: u64,
    pub model_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ProviderProfile {
    pub fn auth_mode(&self) -> AuthMode {
        self.auth.unwrap_or_else(|| self.protocol.default_auth())
    }

    pub fn pricing_or_default(&self) -> Pricing {
        self.pricing
            .unwrap_or_else(|| self.protocol.default_pricing())
    }

    /// Resolve a requested model to a wire model id.
    ///
    /// Empty or `"default"` selects the profile's primary model. Other values
    /// go through `model_map` first, so a UI label can point at a concrete id.
    pub fn resolve_model(&self, requested: &str) -> String {
        let requested = requested.trim();
        if requested.is_empty() || requested.eq_ignore_ascii_case("default") {
            return self.model.clone();
        }
        self.model_map
            .get(requested)
            .cloned()
            .unwrap_or_else(|| requested.to_string())
    }

    pub fn has_key(&self) -> bool {
        match &self.api_key {
            Some(secret) => secret.is_configured(),
            None => self.auth_mode() == AuthMode::None,
        }
    }
}

/// Patch-style input used by both the settings UI and the headless CLI.
/// Omitted fields keep their current value on update.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInput {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    /// `None` keeps the current secret; `Some("")` clears it.
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub models: Option<Vec<String>>,
    #[serde(default)]
    pub model_map: Option<HashMap<String, String>>,
    #[serde(default)]
    pub auth: Option<String>,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default)]
    pub pricing: Option<Pricing>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub sort_index: Option<usize>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub icon_color: Option<String>,
    #[serde(default)]
    pub website_url: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderFile {
    pub version: u32,
    pub active_id: Option<String>,
    pub providers: Vec<ProviderProfile>,
}

impl Default for ProviderFile {
    fn default() -> Self {
        Self {
            version: 1,
            active_id: None,
            providers: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_aliases_parse() {
        assert_eq!(
            Protocol::parse("anthropic").unwrap(),
            Protocol::AnthropicMessages
        );
        assert_eq!(Protocol::parse("Chat").unwrap(), Protocol::OpenAiChat);
        assert_eq!(
            Protocol::parse("responses").unwrap(),
            Protocol::OpenAiResponses
        );
        assert!(Protocol::parse("grpc").is_err());
    }

    #[test]
    fn bare_secret_becomes_literal_and_is_redacted() {
        let secret = SecretRef::normalize("sk-test").unwrap();
        assert_eq!(secret.kind(), "literal");
        assert_eq!(secret.hint(), "literal:***");
        assert_eq!(secret.resolve().unwrap(), "sk-test");
    }

    #[test]
    fn empty_secret_clears() {
        assert!(SecretRef::normalize("   ").is_none());
    }

    #[test]
    fn model_mapping_resolves_default_and_roles() {
        let mut profile = ProviderProfile {
            id: "p".into(),
            name: "p".into(),
            protocol: Protocol::OpenAiChat,
            base_url: "https://example.com".into(),
            api_key: None,
            model: "gpt-5".into(),
            models: vec![],
            model_map: HashMap::from([("fast".to_string(), "gpt-5-mini".to_string())]),
            auth: Some(AuthMode::Bearer),
            headers: HashMap::new(),
            pricing: None,
            max_output_tokens: None,
            temperature: None,
            enabled: true,
            sort_index: 0,
            icon: None,
            icon_color: None,
            website_url: None,
            notes: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert_eq!(profile.resolve_model("default"), "gpt-5");
        assert_eq!(profile.resolve_model("fast"), "gpt-5-mini");
        assert_eq!(profile.resolve_model("gpt-5-nano"), "gpt-5-nano");
        profile.api_key = Some(SecretRef::literal("x"));
        assert!(profile.has_key());
    }
}
