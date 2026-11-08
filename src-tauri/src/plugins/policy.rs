use std::sync::Arc;

use serde_json::Value;

use crate::contracts::ports::{PolicyDecision, PolicyEffect, PolicyInput, PolicyPort, SandboxPort};
use crate::contracts::SLOT_POLICY_EVALUATE;
use crate::kernel::lifecycle::{Plugin, PluginContext};
use crate::kernel::registry::Contribution;

/// Default policy for the MVP.
///
/// - reads inside the workspace: allow
/// - writes inside the workspace: ask (human approval)
/// - shell execution: ask, with an explicit deny list for destructive commands
/// - anything else / outside the workspace: deny
/// - no matching rule = deny (least privilege)
pub struct DefaultPolicyPlugin {
    sandbox: Arc<dyn SandboxPort>,
}

impl DefaultPolicyPlugin {
    pub fn new(sandbox: Arc<dyn SandboxPort>) -> Self {
        Self { sandbox }
    }

    fn inside(&self, input: &PolicyInput) -> Result<(), String> {
        let requested = input
            .args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".");
        self.sandbox.resolve_path(&input.workspace, requested)?;
        Ok(())
    }
}

impl Plugin for DefaultPolicyPlugin {
    fn id(&self) -> &'static str {
        "plugin-policy"
    }

    fn activate(&self, _ctx: &PluginContext) -> Result<Vec<Contribution>, String> {
        Ok(vec![Contribution {
            plugin_id: self.id().to_string(),
            slot: SLOT_POLICY_EVALUATE.to_string(),
            id: "default".to_string(),
        }])
    }
}

const DANGEROUS_PATTERNS: &[&str] = &[
    "rm -rf /",
    "rm -rf ~",
    "format c:",
    "format d:",
    "del /f /s /q c:\\",
    "rd /s /q c:\\",
    "shutdown",
    "mkfs",
    "dd if=",
    ":(){",
    "> /dev/sda",
];

impl PolicyPort for DefaultPolicyPlugin {
    fn evaluate(&self, input: &PolicyInput) -> PolicyDecision {
        match input.tool_id.as_str() {
            "fs_list" | "fs_read" => match self.inside(input) {
                Ok(()) => PolicyDecision {
                    effect: PolicyEffect::Allow,
                    rationale: "工作区内只读访问".to_string(),
                },
                Err(reason) => PolicyDecision {
                    effect: PolicyEffect::Deny,
                    rationale: reason,
                },
            },
            "fs_write" => match self.inside(input) {
                Ok(()) => PolicyDecision {
                    effect: PolicyEffect::Ask,
                    rationale: "写入工作区会修改数据，需要人工审批（旧内容会备份）".to_string(),
                },
                Err(reason) => PolicyDecision {
                    effect: PolicyEffect::Deny,
                    rationale: reason,
                },
            },
            "shell_exec" => {
                let command = input
                    .args
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if command.trim().is_empty() {
                    return PolicyDecision {
                        effect: PolicyEffect::Deny,
                        rationale: "空命令".to_string(),
                    };
                }
                let lowered = command.to_lowercase();
                if DANGEROUS_PATTERNS
                    .iter()
                    .any(|pattern| lowered.contains(pattern))
                {
                    return PolicyDecision {
                        effect: PolicyEffect::Deny,
                        rationale: "命令命中危险模式，默认拒绝".to_string(),
                    };
                }
                PolicyDecision {
                    effect: PolicyEffect::Ask,
                    rationale: "命令执行不可逆，需要人工审批".to_string(),
                }
            }
            other => PolicyDecision {
                effect: PolicyEffect::Deny,
                rationale: format!("没有匹配 {other} 的策略，默认拒绝"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::sandbox::FsSandboxPlugin;
    use serde_json::json;
    use std::path::PathBuf;

    fn ws() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("phi-policy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn policy() -> DefaultPolicyPlugin {
        DefaultPolicyPlugin::new(Arc::new(FsSandboxPlugin))
    }

    fn input(tool: &str, args: Value) -> PolicyInput {
        PolicyInput {
            tool_id: tool.to_string(),
            args,
            workspace: ws(),
        }
    }

    #[test]
    fn read_inside_is_allowed() {
        let d = policy().evaluate(&input("fs_read", json!({"path": "a.txt"})));
        assert_eq!(d.effect, PolicyEffect::Allow);
    }

    #[test]
    fn read_outside_is_denied() {
        let d = policy().evaluate(&input("fs_read", json!({"path": "../a.txt"})));
        assert_eq!(d.effect, PolicyEffect::Deny);
    }

    #[test]
    fn write_inside_asks() {
        let d = policy().evaluate(&input("fs_write", json!({"path": "a.txt", "content": "x"})));
        assert_eq!(d.effect, PolicyEffect::Ask);
    }

    #[test]
    fn shell_asks_by_default() {
        let d = policy().evaluate(&input("shell_exec", json!({"command": "echo hi"})));
        assert_eq!(d.effect, PolicyEffect::Ask);
    }

    #[test]
    fn destructive_shell_is_denied() {
        let d = policy().evaluate(&input("shell_exec", json!({"command": "rm -rf /"})));
        assert_eq!(d.effect, PolicyEffect::Deny);
    }

    #[test]
    fn unknown_tool_is_denied() {
        let d = policy().evaluate(&input("whatever", json!({})));
        assert_eq!(d.effect, PolicyEffect::Deny);
    }
}
