use std::sync::Arc;

use serde_json::Value;

use phi_contracts::SLOT_POLICY_EVALUATE;
use phi_contracts::ports::{PolicyDecision, PolicyEffect, PolicyInput, PolicyPort, SandboxPort};
use phi_i18n as i18n;
use phi_kernel::lifecycle::{Plugin, PluginContext};
use phi_kernel::registry::Contribution;

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
                    rationale: i18n::t("Read-only access inside the workspace"),
                },
                Err(reason) => PolicyDecision {
                    effect: PolicyEffect::Deny,
                    rationale: reason,
                },
            },
            "fs_write" => match self.inside(input) {
                Ok(()) => PolicyDecision {
                    effect: PolicyEffect::Ask,
                    rationale: i18n::t(
                        "Writing to the workspace modifies data and requires human approval (previous content is backed up)",
                    ),
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
                        rationale: i18n::t("Empty command"),
                    };
                }
                let lowered = command.to_lowercase();
                if DANGEROUS_PATTERNS
                    .iter()
                    .any(|pattern| lowered.contains(pattern))
                {
                    return PolicyDecision {
                        effect: PolicyEffect::Deny,
                        rationale: i18n::t(
                            "Command matches a dangerous pattern; denied by default",
                        ),
                    };
                }
                PolicyDecision {
                    effect: PolicyEffect::Ask,
                    rationale: i18n::t(
                        "Command execution is irreversible and requires human approval",
                    ),
                }
            }
            other => PolicyDecision {
                effect: PolicyEffect::Deny,
                rationale: i18n::t_args(
                    "No policy matches {tool}; denied by default",
                    &[("tool", other.to_string())],
                ),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::FsSandboxPlugin;
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
