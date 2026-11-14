//! Model gateway: provider selection, protocol dispatch, streaming, metering.
//!
//! Every model call in the node goes through this plugin. It resolves the
//! active provider profile, builds the wire request, streams deltas to the
//! registered shell sink (ephemeral, not journaled), and returns the final
//! response for the durable assistant message.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::model_protocol::{
    ProtocolResponse, StreamDelta, build_http, conservative_request_tokens, decoder, model_error,
    models_url, parse_http_error, parse_response,
};
use crate::provider_store::{ProviderStore, ResolvedProvider};
use phi_contracts::SLOT_MODEL_INVOKE;
use phi_contracts::ports::{ModelError, ModelPort, ModelRequest, ModelResponse, ModelStatus};
use phi_contracts::provider::{AuthMode, ProviderProfile};
use phi_contracts::stream::{ModelStreamEvent, StreamHub};
use phi_domain::message::{ContentBlock, Role};
use phi_i18n as i18n;
use phi_kernel::lifecycle::{Plugin, PluginContext};
use phi_kernel::registry::Contribution;

pub struct ModelGateway {
    client: reqwest::Client,
    providers: Arc<ProviderStore>,
    streams: Arc<StreamHub>,
}

impl ModelGateway {
    pub fn new(providers: Arc<ProviderStore>, streams: Arc<StreamHub>) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            // Streaming responses can legitimately outlive a short request
            // timeout; the run's wall-clock deadline is the real limit.
            .timeout(Duration::from_secs(900))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            providers,
            streams,
        }
    }

    fn compute_cost(
        &self,
        profile: &ProviderProfile,
        input_tokens: u64,
        output_tokens: u64,
    ) -> f64 {
        let pricing = profile.pricing_or_default();
        input_tokens as f64 / 1_000_000.0 * pricing.input_per_mtok
            + output_tokens as f64 / 1_000_000.0 * pricing.output_per_mtok
    }

    async fn invoke_resolved(
        &self,
        resolved: ResolvedProvider,
        req: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelResponse, ModelError> {
        let profile = resolved.profile;
        let stream = req.stream && self.streams.is_enabled();
        let plan = build_http(&profile, &req, stream)?;
        let mut builder = self.client.post(&plan.url).json(&plan.body);
        for (key, value) in &plan.headers {
            builder = builder.header(key, value);
        }

        let response = tokio::select! {
            result = builder.send() => result.map_err(|error| ModelError {
                message: i18n::t_args(
                    "network: request failed: {error}",
                    &[("error", error.to_string())],
                ),
                retryable: true,
            })?,
            _ = cancel.cancelled() => return Err(cancelled_error()),
        };

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(parse_http_error(status.as_u16(), &body));
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();

        if stream && content_type.contains("text/event-stream") {
            return self.consume_stream(response, profile, &req, cancel).await;
        }

        let raw = response.text().await.map_err(|error| ModelError {
            message: i18n::t_args(
                "network: failed to read response: {error}",
                &[("error", error.to_string())],
            ),
            retryable: true,
        })?;
        let value: Value = serde_json::from_str(&raw).map_err(|error| ModelError {
            message: i18n::t_args(
                "invalid_response: response is not valid JSON: {error}",
                &[("error", error.to_string())],
            ),
            retryable: false,
        })?;
        let parsed = parse_response(profile.protocol, &value)?;
        Ok(self.finish_response(&profile, &req, parsed))
    }

    async fn consume_stream(
        &self,
        mut response: reqwest::Response,
        profile: ProviderProfile,
        req: &ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelResponse, ModelError> {
        let mut buffer = SseBuffer::default();
        let mut decoder = decoder(profile.protocol);
        let mut done_emitted = false;

        loop {
            let chunk = tokio::select! {
                result = response.chunk() => result.map_err(|error| ModelError {
                    message: i18n::t_args(
                        "network: failed to read stream: {error}",
                        &[("error", error.to_string())],
                    ),
                    retryable: true,
                })?,
                _ = cancel.cancelled() => return Err(cancelled_error()),
            };
            let Some(bytes) = chunk else {
                break;
            };
            buffer.push(&bytes);
            while let Some((event, data)) = buffer.next_event() {
                for delta in decoder.feed(&event, &data)? {
                    if matches!(delta, StreamDelta::Done) {
                        done_emitted = true;
                    }
                    self.publish_delta(req, delta);
                }
            }
        }

        // Some servers end the stream without a trailing blank line.
        buffer.push(b"\n\n");
        while let Some((event, data)) = buffer.next_event() {
            for delta in decoder.feed(&event, &data)? {
                if matches!(delta, StreamDelta::Done) {
                    done_emitted = true;
                }
                self.publish_delta(req, delta);
            }
        }

        let parsed = decoder.finish()?;
        if !done_emitted {
            self.publish_delta(req, StreamDelta::Done);
        }
        Ok(self.finish_response(&profile, req, parsed))
    }

    fn finish_response(
        &self,
        profile: &ProviderProfile,
        req: &ModelRequest,
        parsed: ProtocolResponse,
    ) -> ModelResponse {
        let (fallback_input, fallback_output) = conservative_request_tokens(req);
        let input_tokens = if parsed.input_tokens == 0 {
            fallback_input
        } else {
            parsed.input_tokens
        };
        let output_tokens = if parsed.output_tokens == 0 {
            fallback_output
        } else {
            parsed.output_tokens
        };
        ModelResponse {
            blocks: parsed.blocks,
            input_tokens,
            output_tokens,
            cost_usd: self.compute_cost(profile, input_tokens, output_tokens),
            finish_reason: parsed.finish_reason,
            estimated_usage: parsed.estimated_usage,
        }
    }

    fn publish_delta(&self, req: &ModelRequest, delta: StreamDelta) {
        if !self.streams.is_enabled() {
            return;
        }
        let event = match delta {
            StreamDelta::Text { index, text } => ModelStreamEvent {
                run_id: req.run_id.clone(),
                call_id: req.call_id.clone(),
                index,
                kind: "text".to_string(),
                text,
                tool_use_id: None,
                tool_name: None,
            },
            StreamDelta::ToolStart { index, id, name } => ModelStreamEvent {
                run_id: req.run_id.clone(),
                call_id: req.call_id.clone(),
                index,
                kind: "tool_use_start".to_string(),
                text: String::new(),
                tool_use_id: Some(id),
                tool_name: Some(name),
            },
            StreamDelta::ToolArgs { index, delta } => ModelStreamEvent {
                run_id: req.run_id.clone(),
                call_id: req.call_id.clone(),
                index,
                kind: "tool_use_delta".to_string(),
                text: delta,
                tool_use_id: None,
                tool_name: None,
            },
            StreamDelta::ToolEnd { index } => ModelStreamEvent {
                run_id: req.run_id.clone(),
                call_id: req.call_id.clone(),
                index,
                kind: "tool_use_end".to_string(),
                text: String::new(),
                tool_use_id: None,
                tool_name: None,
            },
            StreamDelta::Done => ModelStreamEvent {
                run_id: req.run_id.clone(),
                call_id: req.call_id.clone(),
                index: 0,
                kind: "done".to_string(),
                text: String::new(),
                tool_use_id: None,
                tool_name: None,
            },
        };
        self.streams.publish(&event);
    }

    /// Deterministic local provider so the loop, policy and tools can be
    /// exercised without an API key.
    fn mock_invoke(&self, req: &ModelRequest) -> Result<ModelResponse, ModelError> {
        let mut tool_results = Vec::new();
        let mut last_user_text = String::new();
        for message in &req.messages {
            for block in &message.blocks {
                match block {
                    ContentBlock::ToolResult { content, .. } => tool_results.push(content.clone()),
                    ContentBlock::Text { text } if message.role == Role::User => {
                        last_user_text = text.clone()
                    }
                    _ => {}
                }
            }
        }

        let (blocks, finish_reason) = if let Some(result) = tool_results.last() {
            (
                vec![ContentBlock::text(i18n::t_args(
                    "(Mock model; no provider configured)\nCompleted one tool call. Latest tool result summary:\n{summary}",
                    &[("summary", truncate(result, 1200))],
                ))],
                "end_turn",
            )
        } else if req.workbench {
            (
                vec![
                    ContentBlock::text(i18n::t(
                        "(Mock model; no provider configured) I will build a workbench view for this task.",
                    )),
                    ContentBlock::ToolUse {
                        id: format!("call_{}", uuid::Uuid::new_v4().simple()),
                        name: "ui_update".to_string(),
                        input: json!({ "view": mock_workbench_view() }),
                    },
                ],
                "tool_use",
            )
        } else {
            let lowercase = last_user_text.to_lowercase();
            let wants_demo_write = lowercase.contains("demo write")
                || lowercase.contains(&i18n::t("demo write").to_lowercase());
            let call = if wants_demo_write {
                ContentBlock::ToolUse {
                    id: format!("call_{}", uuid::Uuid::new_v4().simple()),
                    name: "fs_write".to_string(),
                    input: json!({
                        "path": "PHI_DEMO.md",
                        "content": i18n::t(
                            "# phi-agent demo\n\nWritten by the mock model to demonstrate human approval.\n"
                        )
                    }),
                }
            } else {
                ContentBlock::ToolUse {
                    id: format!("call_{}", uuid::Uuid::new_v4().simple()),
                    name: "fs_list".to_string(),
                    input: json!({ "path": "." }),
                }
            };
            (
                vec![
                    ContentBlock::text(i18n::t(
                        "(Mock model; no provider configured) I will call a tool to inspect the workspace. Configure a provider to run real coding tasks.",
                    )),
                    call,
                ],
                "tool_use",
            )
        };

        if req.stream && self.streams.is_enabled() {
            let mut index = 0u32;
            for block in &blocks {
                match block {
                    ContentBlock::Text { text } => {
                        self.publish_delta(
                            req,
                            StreamDelta::Text {
                                index,
                                text: text.clone(),
                            },
                        );
                    }
                    ContentBlock::ToolUse { id, name, input } => {
                        self.publish_delta(
                            req,
                            StreamDelta::ToolStart {
                                index,
                                id: id.clone(),
                                name: name.clone(),
                            },
                        );
                        self.publish_delta(
                            req,
                            StreamDelta::ToolArgs {
                                index,
                                delta: input.to_string(),
                            },
                        );
                        self.publish_delta(req, StreamDelta::ToolEnd { index });
                    }
                    _ => {}
                }
                index += 1;
            }
            self.publish_delta(req, StreamDelta::Done);
        }

        Ok(ModelResponse {
            blocks,
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            finish_reason: finish_reason.to_string(),
            estimated_usage: false,
        })
    }

    fn resolve_for(&self, id: Option<&str>) -> Result<ResolvedProvider, String> {
        match id {
            Some(id) => self.providers.resolve(id),
            None => self.providers.resolve_active()?.ok_or_else(|| {
                i18n::t(
                    "No provider is configured; add a provider or set the environment variables",
                )
            }),
        }
    }

    pub async fn list_models(&self, id: Option<&str>) -> Result<(String, Vec<String>), String> {
        let resolved = self.resolve_for(id)?;
        let profile = resolved.profile;
        let url = models_url(&profile);
        let mut request = self.client.get(&url);
        if let Some(key) = resolved.api_key.as_deref() {
            request = match profile.auth_mode() {
                AuthMode::XApiKey => request.header("x-api-key", key),
                AuthMode::Bearer => request.header("authorization", format!("Bearer {key}")),
                AuthMode::None => request,
            };
        }
        for (key, value) in &profile.headers {
            request = request.header(key, value);
        }
        let response = request.send().await.map_err(|error| {
            i18n::t_args(
                "network: failed to list models: {error}",
                &[("error", error.to_string())],
            )
        })?;
        let status = response.status();
        let raw = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(parse_http_error(status.as_u16(), &raw).message);
        }
        let value: Value = serde_json::from_str(&raw).map_err(|error| {
            i18n::t_args(
                "invalid_response: model list is not valid JSON: {error}",
                &[("error", error.to_string())],
            )
        })?;
        let mut models = Vec::new();
        if let Some(data) = value["data"].as_array() {
            for item in data {
                if let Some(id) = item["id"].as_str() {
                    models.push(id.to_string());
                } else if let Some(name) = item["name"].as_str() {
                    models.push(name.to_string());
                }
            }
        } else if let Some(data) = value["models"].as_array() {
            for item in data {
                if let Some(id) = item["id"].as_str() {
                    models.push(id.to_string());
                } else if let Some(id) = item.as_str() {
                    models.push(id.to_string());
                }
            }
        } else if let Some(data) = value.as_array() {
            for item in data {
                if let Some(id) = item.as_str() {
                    models.push(id.to_string());
                }
            }
        }
        models.sort();
        models.dedup();
        if !models.is_empty() {
            self.providers.mark_models(&profile.id, models.clone())?;
        }
        Ok((profile.id, models))
    }

    pub async fn test_provider(
        &self,
        id: Option<&str>,
    ) -> phi_contracts::provider::ProviderTestView {
        let started = Instant::now();
        match self.list_models(id).await {
            Ok((_, models)) => phi_contracts::provider::ProviderTestView {
                ok: true,
                latency_ms: started.elapsed().as_millis() as u64,
                model_count: models.len(),
                error: None,
            },
            Err(error) => phi_contracts::provider::ProviderTestView {
                ok: false,
                latency_ms: started.elapsed().as_millis() as u64,
                model_count: 0,
                error: Some(error),
            },
        }
    }
}

/// A small but complete view tree, so the workbench path is exercised without
/// a provider: layout, card, text, progress, badge, button and table.
fn mock_workbench_view() -> Value {
    json!({
        "version": 1,
        "title": i18n::t("Workbench"),
        "root": {
            "kind": "stack",
            "props": { "gap": "md" },
            "children": [
                {
                    "kind": "card",
                    "props": {
                        "title": i18n::t("Task overview"),
                        "subtitle": i18n::t("Mock workbench")
                    },
                    "children": [
                        {
                            "kind": "text",
                            "props": {
                                "text": i18n::t("This view is plain data. The agent can replace it with tables, charts, forms and actions as the task progresses.")
                            }
                        },
                        {
                            "kind": "progress",
                            "props": { "value": 40, "label": i18n::t("Example progress") }
                        },
                        {
                            "kind": "row",
                            "props": { "gap": "sm" },
                            "children": [
                                { "kind": "badge", "props": { "text": i18n::t("offline"), "tone": "neutral" } },
                                {
                                    "kind": "button",
                                    "props": {
                                        "label": i18n::t("Inspect workspace"),
                                        "action": {
                                            "kind": "message",
                                            "text": i18n::t("List the workspace and summarize it.")
                                        }
                                    }
                                }
                            ]
                        }
                    ]
                },
                {
                    "kind": "card",
                    "props": { "title": i18n::t("Recent tool calls") },
                    "children": [
                        {
                            "kind": "table",
                            "props": {
                                "columns": [i18n::t("Step"), i18n::t("Status")],
                                "rows": [["1", i18n::t("ok")], ["2", i18n::t("pending")]]
                            }
                        }
                    ]
                }
            ]
        }
    })
}

impl Plugin for ModelGateway {
    fn id(&self) -> &'static str {
        "plugin-model-gw"
    }

    fn activate(&self, _ctx: &PluginContext) -> Result<Vec<Contribution>, String> {
        Ok(vec![Contribution {
            plugin_id: self.id().to_string(),
            slot: SLOT_MODEL_INVOKE.to_string(),
            id: "multi-protocol".to_string(),
        }])
    }
}

#[async_trait]
impl ModelPort for ModelGateway {
    fn status(&self) -> ModelStatus {
        match self.providers.active() {
            Some(profile) => {
                let configured = profile.has_key();
                ModelStatus {
                    provider: profile.name.clone(),
                    model: profile.model.clone(),
                    configured,
                    provider_id: Some(profile.id.clone()),
                    protocol: profile.protocol.as_str().to_string(),
                    base_url: Some(profile.base_url.clone()),
                    has_key: profile
                        .api_key
                        .as_ref()
                        .map(|key| key.is_configured())
                        .unwrap_or(profile.auth_mode() == AuthMode::None),
                }
            }
            None => ModelStatus {
                provider: "mock".to_string(),
                model: "mock".to_string(),
                configured: false,
                provider_id: None,
                protocol: "mock".to_string(),
                base_url: None,
                has_key: false,
            },
        }
    }

    fn set_api_key(&self, key: &str) -> Result<ModelStatus, String> {
        self.providers.set_api_key(key)?;
        Ok(self.status())
    }

    fn estimate_cost(&self, input_chars: usize, input_bytes: usize, max_output_tokens: u32) -> f64 {
        let Some(profile) = self.providers.active() else {
            return 0.0;
        };
        if !profile.has_key() && profile.auth_mode() != AuthMode::None {
            return 0.0;
        }
        let pricing = profile.pricing_or_default();
        let input_tokens = input_chars.max(input_bytes) as f64 + 256.0;
        input_tokens / 1_000_000.0 * pricing.input_per_mtok
            + max_output_tokens as f64 / 1_000_000.0 * pricing.output_per_mtok
    }

    async fn invoke(
        &self,
        req: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelResponse, ModelError> {
        let resolved = match self.providers.resolve_active() {
            Ok(Some(resolved)) => resolved,
            Ok(None) => return self.mock_invoke(&req),
            Err(error) => {
                // A configured-but-broken provider must be visible, not
                // silently replaced by the mock.
                return Err(model_error(format!("provider: {error}")));
            }
        };
        self.invoke_resolved(resolved, req, cancel).await
    }
}

fn cancelled_error() -> ModelError {
    ModelError {
        message: "cancelled".to_string(),
        retryable: false,
    }
}

#[derive(Default)]
struct SseBuffer {
    buf: Vec<u8>,
}

impl SseBuffer {
    fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    fn next_event(&mut self) -> Option<(String, String)> {
        loop {
            let (position, delimiter) = earliest_frame(&self.buf)?;
            let frame: Vec<u8> = self.buf.drain(..position).collect();
            self.buf.drain(..delimiter);
            if let Some(event) = parse_frame(&frame) {
                return Some(event);
            }
        }
    }
}

fn earliest_frame(buf: &[u8]) -> Option<(usize, usize)> {
    let newline = find_subslice(buf, b"\n\n").map(|position| (position, 2));
    let crlf = find_subslice(buf, b"\r\n\r\n").map(|position| (position, 4));
    match (newline, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn parse_frame(frame: &[u8]) -> Option<(String, String)> {
    let text = String::from_utf8_lossy(frame);
    let mut event = String::new();
    let mut data = String::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("event:") {
            event = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("data:") {
            let rest = rest.strip_prefix(' ').unwrap_or(rest);
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest);
        }
    }
    if event.is_empty() && data.is_empty() {
        None
    } else {
        Some((event, data))
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(max_chars).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_buffer_handles_split_frames_and_crlf() {
        let mut buffer = SseBuffer::default();
        buffer.push(b"event: message_start\ndata: {\"a\":1}\n\n");
        let (event, data) = buffer.next_event().unwrap();
        assert_eq!(event, "message_start");
        assert_eq!(data, "{\"a\":1}");

        buffer.push(b"data: {\"b\":");
        assert!(buffer.next_event().is_none());
        buffer.push(b"2}\r\n\r\n");
        let (event, data) = buffer.next_event().unwrap();
        assert_eq!(event, "");
        assert_eq!(data, "{\"b\":2}");
    }

    #[test]
    fn sse_buffer_ignores_comments_and_heartbeats() {
        let mut buffer = SseBuffer::default();
        buffer.push(b": ping\n\ndata: [DONE]\n\n");
        let (_, data) = buffer.next_event().unwrap();
        assert_eq!(data, "[DONE]");
    }

    #[tokio::test]
    async fn mock_runs_without_provider() {
        let dir = std::env::temp_dir().join(format!("phi-gateway-{}", uuid::Uuid::new_v4()));
        let providers = Arc::new(ProviderStore::load(dir.join("providers.json")));
        let gateway = ModelGateway::new(providers, Arc::new(StreamHub::default()));
        let req = ModelRequest {
            model: "default".into(),
            system: "hello".into(),
            messages: vec![phi_domain::message::ChatMessage::user_text("hi")],
            tools: Vec::new(),
            max_output_tokens: 100,
            call_id: "call".into(),
            run_id: "run".into(),
            stream: false,
            workbench: false,
        };
        let response = gateway.invoke(req, CancellationToken::new()).await.unwrap();
        assert!(
            response
                .blocks
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolUse { .. }))
        );
    }
}
