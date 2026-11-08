use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::contracts::ports::{
    ModelError, ModelPort, ModelRequest, ModelResponse, ModelStatus,
};
use crate::contracts::SLOT_MODEL_INVOKE;
use crate::domain::message::{ChatMessage, ContentBlock, Role};
use crate::kernel::lifecycle::{Plugin, PluginContext};
use crate::kernel::ports::IdPort;
use crate::kernel::registry::Contribution;

#[derive(Clone)]
struct GatewayConfig {
    api_key: Option<String>,
    base_url: String,
    model: String,
    input_price_per_mtok: f64,
    output_price_per_mtok: f64,
}

impl GatewayConfig {
    fn from_env() -> Self {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .or_else(|_| std::env::var("PHI_API_KEY"))
            .ok()
            .filter(|key| !key.trim().is_empty());
        Self {
            api_key,
            base_url: std::env::var("PHI_ANTHROPIC_BASE_URL")
                .unwrap_or_else(|_| "https://api.anthropic.com".to_string()),
            model: std::env::var("PHI_MODEL")
                .unwrap_or_else(|_| "claude-sonnet-5".to_string()),
            input_price_per_mtok: std::env::var("PHI_PRICE_INPUT_PER_MTOK")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3.0),
            output_price_per_mtok: std::env::var("PHI_PRICE_OUTPUT_PER_MTOK")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(15.0),
        }
    }
}

pub struct ModelGateway {
    client: reqwest::Client,
    config: RwLock<GatewayConfig>,
    ids: Arc<dyn IdPort>,
}

impl ModelGateway {
    pub fn new(ids: Arc<dyn IdPort>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(180))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            config: RwLock::new(GatewayConfig::from_env()),
            ids,
        }
    }

    fn config(&self) -> GatewayConfig {
        self.config.read().unwrap().clone()
    }

    fn compute_cost(&self, config: &GatewayConfig, input_tokens: u64, output_tokens: u64) -> f64 {
        input_tokens as f64 / 1_000_000.0 * config.input_price_per_mtok
            + output_tokens as f64 / 1_000_000.0 * config.output_price_per_mtok
    }

    async fn anthropic_invoke(
        &self,
        req: ModelRequest,
        config: GatewayConfig,
        api_key: String,
        cancel: CancellationToken,
    ) -> Result<ModelResponse, ModelError> {
        let (estimated_input_tokens, estimated_output_tokens) = conservative_request_tokens(&req);
        let messages: Vec<Value> = req
            .messages
            .iter()
            .map(message_to_anthropic)
            .collect();

        let mut body = json!({
            "model": req.model,
            "max_tokens": req.max_output_tokens,
            "system": req.system,
            "messages": messages,
        });
        if !req.tools.is_empty() {
            let tools: Vec<Value> = req
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": tool.input_schema,
                    })
                })
                .collect();
            body["tools"] = Value::Array(tools);
        }

        let url = format!("{}/v1/messages", config.base_url.trim_end_matches('/'));
        let response = tokio::select! {
            result = self
                .client
                .post(url)
                .header("x-api-key", api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&body)
                .send() => result.map_err(|e| ModelError {
                    message: format!("模型请求失败: {e}"),
                    retryable: true,
                })?,
            _ = cancel.cancelled() => {
                return Err(ModelError {
                    message: "cancelled".to_string(),
                    retryable: false,
                })
            }
        };

        let status = response.status();
        let raw = response
            .text()
            .await
            .map_err(|e| ModelError {
                message: format!("读取模型响应失败: {e}"),
                retryable: true,
            })?;
        if !status.is_success() {
            return Err(ModelError {
                message: format!("模型返回 {status}: {}", truncate(&raw, 800)),
                retryable: status.as_u16() == 429 || status.is_server_error(),
            });
        }

        let value: Value = serde_json::from_str(&raw).map_err(|e| ModelError {
            message: format!("模型响应不是合法 JSON: {e}"),
            retryable: false,
        })?;

        let mut blocks = Vec::new();
        if let Some(content) = value["content"].as_array() {
            for item in content {
                match item["type"].as_str() {
                    Some("text") => {
                        if let Some(text) = item["text"].as_str() {
                            blocks.push(ContentBlock::text(text));
                        }
                    }
                    Some("tool_use") => {
                        blocks.push(ContentBlock::ToolUse {
                            id: item["id"].as_str().unwrap_or_default().to_string(),
                            name: item["name"].as_str().unwrap_or_default().to_string(),
                            input: item["input"].clone(),
                        });
                    }
                    _ => {}
                }
            }
        }
        if blocks.is_empty() {
            blocks.push(ContentBlock::text("（模型没有返回内容）"));
        }

        // A compatible proxy may omit usage. Never treat that as zero cost:
        // fall back to the conservative request estimate and let the caller
        // settle at the reserved amount (ADR / agent-loop §3).
        let usage_input = value
            .get("usage")
            .and_then(|usage| usage.get("input_tokens"))
            .and_then(|v| v.as_u64());
        let usage_output = value
            .get("usage")
            .and_then(|usage| usage.get("output_tokens"))
            .and_then(|v| v.as_u64());
        let estimated_usage = usage_input.is_none() || usage_output.is_none();
        let input_tokens = usage_input.unwrap_or(estimated_input_tokens);
        let output_tokens = usage_output.unwrap_or(estimated_output_tokens);
        let cost_usd = self.compute_cost(&config, input_tokens, output_tokens);

        Ok(ModelResponse {
            blocks,
            input_tokens,
            output_tokens,
            cost_usd,
            finish_reason: value["stop_reason"]
                .as_str()
                .unwrap_or("end_turn")
                .to_string(),
            estimated_usage,
        })
    }

    /// Deterministic local provider so the loop, policy and tools can be
    /// exercised without an API key.
    fn mock_invoke(&self, req: ModelRequest) -> Result<ModelResponse, ModelError> {
        let mut tool_results = Vec::new();
        let mut last_user_text = String::new();
        for message in &req.messages {
            for block in &message.blocks {
                match block {
                    ContentBlock::ToolResult { content, .. } => {
                        tool_results.push(content.clone())
                    }
                    ContentBlock::Text { text } if message.role == Role::User => {
                        last_user_text = text.clone()
                    }
                    _ => {}
                }
            }
        }

        let (blocks, finish_reason) = if let Some(result) = tool_results.last() {
            (
                vec![ContentBlock::text(format!(
                    "（Mock 模型，未配置 ANTHROPIC_API_KEY）\n已跑通一次工具调用。最近一次工具结果摘要：\n{}",
                    truncate(result, 1200)
                ))],
                "end_turn",
            )
        } else {
            let lowercase = last_user_text.to_lowercase();
            let wants_demo_write =
                lowercase.contains("演示写") || lowercase.contains("demo write");
            let call = if wants_demo_write {
                ContentBlock::ToolUse {
                    id: self.ids.new_id("call"),
                    name: "fs_write".to_string(),
                    input: json!({
                        "path": "PHI_DEMO.md",
                        "content": "# phi-agent demo\n\n由 Mock 模型写入，用于演示人工审批。\n"
                    }),
                }
            } else {
                ContentBlock::ToolUse {
                    id: self.ids.new_id("call"),
                    name: "fs_list".to_string(),
                    input: json!({ "path": "." }),
                }
            };
            (
                vec![
                    ContentBlock::text(
                        "（Mock 模型，未配置 ANTHROPIC_API_KEY）我先调用一个工具看看工作区。配置密钥后可执行真实编码任务。",
                    ),
                    call,
                ],
                "tool_use",
            )
        };

        Ok(ModelResponse {
            blocks,
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            finish_reason: finish_reason.to_string(),
            estimated_usage: false,
        })
    }
}

impl Plugin for ModelGateway {
    fn id(&self) -> &'static str {
        "plugin-model-gw"
    }

    fn activate(&self, _ctx: &PluginContext) -> Result<Vec<Contribution>, String> {
        Ok(vec![Contribution {
            plugin_id: self.id().to_string(),
            slot: SLOT_MODEL_INVOKE.to_string(),
            id: "anthropic+mock".to_string(),
        }])
    }
}

#[async_trait]
impl ModelPort for ModelGateway {
    fn status(&self) -> ModelStatus {
        let config = self.config();
        let configured = config.api_key.is_some();
        ModelStatus {
            provider: if configured { "anthropic" } else { "mock" }.to_string(),
            model: if configured {
                config.model
            } else {
                "mock".to_string()
            },
            configured,
        }
    }

    fn set_api_key(&self, key: &str) -> ModelStatus {
        {
            let mut config = self.config.write().unwrap();
            config.api_key = if key.trim().is_empty() {
                None
            } else {
                Some(key.trim().to_string())
            };
        }
        self.status()
    }

    fn estimate_cost(&self, input_chars: usize, input_bytes: usize, max_output_tokens: u32) -> f64 {
        let config = self.config();
        if config.api_key.is_none() {
            return 0.0;
        }
        // Tokenizers map byte sequences to tokens, so the UTF-8 byte count is
        // a hard upper bound on input tokens. The old chars/4 estimate badly
        // underestimated CJK (3 bytes per character) and could overspend a
        // "hard" budget after the fact.
        let input_tokens = input_chars.max(input_bytes) as f64 + 256.0;
        input_tokens / 1_000_000.0 * config.input_price_per_mtok
            + max_output_tokens as f64 / 1_000_000.0 * config.output_price_per_mtok
    }

    async fn invoke(
        &self,
        req: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelResponse, ModelError> {
        let config = self.config();
        match config.api_key.clone() {
            Some(api_key) => self.anthropic_invoke(req, config, api_key, cancel).await,
            None => self.mock_invoke(req),
        }
    }
}

fn conservative_request_tokens(req: &ModelRequest) -> (u64, u64) {
    let mut bytes = req.system.len() + 256;
    for message in &req.messages {
        bytes += 32;
        for block in &message.blocks {
            bytes += match block {
                ContentBlock::Text { text } => text.len(),
                ContentBlock::ToolUse { input, .. } => input.to_string().len(),
                ContentBlock::ToolResult { content, .. } => content.len(),
            };
        }
    }
    for tool in &req.tools {
        bytes += 64 + tool.description.len() + tool.input_schema.to_string().len();
    }
    // Byte count is a hard upper bound on tokens for a byte-level tokenizer.
    let input_tokens = bytes as u64 + 256;
    (input_tokens, req.max_output_tokens as u64)
}

fn message_to_anthropic(message: &ChatMessage) -> Value {
    let content: Vec<Value> = message
        .blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => json!({ "type": "text", "text": text }),
            ContentBlock::ToolUse { id, name, input } => json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": input,
            }),
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": content,
                "is_error": is_error,
            }),
        })
        .collect();
    let role = match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    json!({
        "role": role,
        "content": content,
    })
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
