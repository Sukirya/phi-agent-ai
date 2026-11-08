use std::path::Path;
use std::sync::Arc;

use crate::contracts::ports::{AssembledContext, ContextPort, SandboxPort};
use crate::contracts::util::truncate_chars;
use crate::contracts::SLOT_CONTEXT_CONTRIBUTOR;
use crate::domain::message::{ChatMessage, ContentBlock, Role};
use crate::kernel::lifecycle::{Plugin, PluginContext};
use crate::kernel::registry::Contribution;

pub struct DefaultContextPlugin {
    sandbox: Arc<dyn SandboxPort>,
}

impl DefaultContextPlugin {
    pub fn new(sandbox: Arc<dyn SandboxPort>) -> Self {
        Self { sandbox }
    }
}

impl Plugin for DefaultContextPlugin {
    fn id(&self) -> &'static str {
        "plugin-context"
    }

    fn activate(&self, _ctx: &PluginContext) -> Result<Vec<Contribution>, String> {
        Ok(vec![Contribution {
            plugin_id: self.id().to_string(),
            slot: SLOT_CONTEXT_CONTRIBUTOR.to_string(),
            id: "default".to_string(),
        }])
    }
}

const MAX_HISTORY_CHARS: usize = 100_000;
const MAX_PROJECT_INSTRUCTIONS: usize = 8_000;

impl ContextPort for DefaultContextPlugin {
    fn assemble(&self, history: Vec<ChatMessage>, workspace: &Path) -> AssembledContext {
        let mut system = String::new();
        system.push_str(
            "You are phi-agent, a local-first coding agent running on the user's machine.\n",
        );
        system.push_str(&format!(
            "Workspace (all tool paths are relative to it): {}\n\n",
            workspace.display()
        ));
        system.push_str(
            "Rules:\n\
             - Use the provided tools to inspect and modify files. Never claim a file changed without a tool call.\n\
             - Reads inside the workspace are automatic. Writes and shell commands require human approval, so state why you need them.\n\
             - Prefer small, focused changes and preserve existing project conventions.\n\
             - After modifying code, run a relevant verification command (test/build/lint) and report the actual result.\n\
             - Do not attempt to access paths outside the workspace; such calls are denied.\n\
             - When the task is complete, reply with a concise summary of changes and verification results.\n\n",
        );

        for name in ["AGENTS.md", "CLAUDE.md", ".phi/AGENTS.md"] {
            // Project instructions are workspace content: resolve them through
            // the sandbox so a junction/symlink cannot smuggle in a file from
            // outside the workspace.
            let path = match self.sandbox.resolve_path(workspace, name) {
                Ok(path) => path,
                Err(_) => continue,
            };
            if let Ok(text) = std::fs::read_to_string(&path) {
                system.push_str(&format!("Project instructions ({name}):\n"));
                system.push_str(&truncate_chars(&text, MAX_PROJECT_INSTRUCTIONS));
                system.push_str("\n\n");
            }
        }

        let messages = trim_history(history);
        AssembledContext { system, messages }
    }
}

fn message_chars(message: &ChatMessage) -> usize {
    message
        .blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text { text } => text.chars().count(),
            ContentBlock::ToolUse { name, input, .. } => {
                name.chars().count() + input.to_string().chars().count()
            }
            ContentBlock::ToolResult { content, .. } => content.chars().count(),
        })
        .sum::<usize>()
        + 32
}

fn trim_history(history: Vec<ChatMessage>) -> Vec<ChatMessage> {
    let mut kept: Vec<ChatMessage> = Vec::new();
    let mut total = 0usize;
    for message in history.into_iter().rev() {
        let size = message_chars(&message);
        if total + size > MAX_HISTORY_CHARS && !kept.is_empty() {
            break;
        }
        total += size;
        kept.push(message);
    }
    kept.reverse();

    // Never start with an orphan tool_result (its tool_use was trimmed away).
    while kept.first().map(|m| {
        m.role == Role::User
            && !m.blocks.is_empty()
            && m.blocks
                .iter()
                .all(|b| matches!(b, ContentBlock::ToolResult { .. }))
    }) == Some(true)
    {
        kept.remove(0);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn drops_orphan_tool_result_prefix() {
        let history = vec![
            ChatMessage {
                role: Role::User,
                blocks: vec![ContentBlock::tool_result("call-1", "result", false)],
            },
            ChatMessage::user_text("hello"),
        ];
        let trimmed = trim_history(history);
        assert_eq!(trimmed.len(), 1);
        assert_eq!(trimmed[0].role, Role::User);
    }

    #[test]
    fn keeps_tool_use_and_result_pairs() {
        let history = vec![ChatMessage {
            role: Role::Assistant,
            blocks: vec![ContentBlock::ToolUse {
                id: "call-1".to_string(),
                name: "fs_list".to_string(),
                input: json!({}),
            }],
        }];
        let trimmed = trim_history(history);
        assert_eq!(trimmed.len(), 1);
    }
}
