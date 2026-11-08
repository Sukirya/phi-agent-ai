use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Stable, key-sorted JSON encoding used by the action digest.
fn canonical_json(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).unwrap_or_default());
                out.push(':');
                canonical_json(&map[*key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical_json(item, out);
            }
            out.push(']');
        }
        _ => out.push_str(&serde_json::to_string(value).unwrap_or_default()),
    }
}

/// Identity of the concrete resource an action will touch.
///
/// Path strings alone are insufficient for approval binding (ADR-0010): the
/// same string can resolve to a different target after a junction/symlink is
/// swapped, and file contents can change between approval and execution.
#[derive(Clone, Debug)]
pub struct ResourceIdentity {
    /// Path exactly as requested by the model.
    pub requested: String,
    /// Canonical path after sandbox resolution.
    pub resolved: String,
    /// `file`, `dir`, `missing`, `denied` or `unknown`.
    pub kind: &'static str,
    /// Hash of the current content (files only).
    pub content_hash: Option<String>,
}

pub fn resource_fingerprint(path: &Path) -> (&'static str, Option<String>) {
    if path.is_file() {
        match std::fs::read(path) {
            Ok(bytes) => ("file", Some(hex::encode(Sha256::digest(&bytes)))),
            Err(_) => ("unreadable", None),
        }
    } else if path.is_dir() {
        ("dir", None)
    } else {
        ("missing", None)
    }
}

/// Approval binds the concrete action: tool identity, workspace identity,
/// normalized arguments and the resolved resource identities. Changing any of
/// them (including file content or a junction target) invalidates the approval.
pub fn action_digest(
    tool_id: &str,
    args: &Value,
    workspace: &Path,
    resources: &[ResourceIdentity],
) -> String {
    let mut canonical = String::new();
    canonical_json(args, &mut canonical);
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let mut resource_text = String::new();
    for resource in resources {
        resource_text.push_str(&format!(
            "{}|{}|{}|{}\n",
            resource.requested,
            resource.resolved,
            resource.kind,
            resource.content_hash.as_deref().unwrap_or("-")
        ));
    }
    let payload = format!(
        "tool={tool_id}|workspace={}|args={canonical}|resources={resource_text}",
        workspace.to_string_lossy()
    );
    let hash = Sha256::digest(payload.as_bytes());
    hex::encode(hash)
}

pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push_str("\n…（已截断）");
    out
}

pub fn args_preview(args: &Value) -> Value {
    let text = serde_json::to_string(args).unwrap_or_default();
    if text.chars().count() <= 2000 {
        args.clone()
    } else {
        Value::String(truncate_chars(&text, 2000))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn digest_is_stable_for_key_order() {
        let ws = PathBuf::from("C:/workspace");
        let a = action_digest(
            "fs_write",
            &json!({"path":"a.txt","content":"x"}),
            &ws,
            &[],
        );
        let b = action_digest(
            "fs_write",
            &json!({"content":"x","path":"a.txt"}),
            &ws,
            &[],
        );
        assert_eq!(a, b);
    }

    #[test]
    fn digest_changes_with_args() {
        let ws = PathBuf::from("C:/workspace");
        let a = action_digest(
            "fs_write",
            &json!({"path":"a.txt","content":"x"}),
            &ws,
            &[],
        );
        let b = action_digest(
            "fs_write",
            &json!({"path":"a.txt","content":"y"}),
            &ws,
            &[],
        );
        assert_ne!(a, b);
    }

    #[test]
    fn digest_changes_when_resource_content_changes() {
        let ws = PathBuf::from("C:/workspace");
        let args = json!({"path":"a.txt","content":"x"});
        let before = action_digest(
            "fs_write",
            &args,
            &ws,
            &[ResourceIdentity {
                requested: "a.txt".to_string(),
                resolved: "C:/workspace/a.txt".to_string(),
                kind: "file",
                content_hash: Some("aaa".to_string()),
            }],
        );
        let after = action_digest(
            "fs_write",
            &args,
            &ws,
            &[ResourceIdentity {
                requested: "a.txt".to_string(),
                resolved: "C:/workspace/a.txt".to_string(),
                kind: "file",
                content_hash: Some("bbb".to_string()),
            }],
        );
        assert_ne!(before, after);
    }
}
