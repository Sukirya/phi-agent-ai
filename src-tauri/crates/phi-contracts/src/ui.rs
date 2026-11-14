//! Workbench view tree: the versioned, declarative UI protocol.
//!
//! The node validates and stores view trees; the shell owns the renderer and
//! decides what each `kind` looks like. Plugins and agents never emit code
//! that runs in the shell.

use serde_json::Value;

/// Bump when the tree shape changes incompatibly. Shells reject unknown
/// versions instead of guessing.
pub const UI_VIEW_VERSION: u64 = 1;
/// Hard cap on a stored view tree. Views are projected into every session
/// snapshot, so an unbounded tree would slow every event refresh.
pub const UI_VIEW_MAX_BYTES: usize = 200_000;
const MAX_DEPTH: usize = 16;
const MAX_NODES: usize = 800;

/// Component kinds the shell knows how to render. Adding a kind is a protocol
/// change: both the validator and the shell registry must learn it together.
pub const UI_NODE_KINDS: &[&str] = &[
    "stack", "row", "grid", "card", "text", "code", "table", "keyvalue", "chart", "form", "field",
    "button", "badge", "progress", "divider",
];

/// Reject malformed or hostile trees before they reach the journal. Errors are
/// fed back to the model as a tool result, so they must be specific.
pub fn validate_view(view: &Value) -> Result<(), String> {
    let size = serde_json::to_vec(view)
        .map_err(|error| format!("view is not serializable: {error}"))?
        .len();
    if size > UI_VIEW_MAX_BYTES {
        return Err(format!(
            "view is too large: {size} bytes (limit {UI_VIEW_MAX_BYTES})"
        ));
    }
    let object = view
        .as_object()
        .ok_or_else(|| "view must be a JSON object".to_string())?;
    let version = object
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| "view.version must be a number".to_string())?;
    if version != UI_VIEW_VERSION {
        return Err(format!(
            "unsupported view.version {version} (expected {UI_VIEW_VERSION})"
        ));
    }
    if let Some(title) = object.get("title") {
        if !title.is_string() {
            return Err("view.title must be a string".to_string());
        }
    }
    let root = object
        .get("root")
        .ok_or_else(|| "view.root is required".to_string())?;
    let mut nodes = 0;
    validate_node(root, 0, &mut nodes)
}

fn validate_node(node: &Value, depth: usize, nodes: &mut usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("view is nested deeper than {MAX_DEPTH} levels"));
    }
    *nodes += 1;
    if *nodes > MAX_NODES {
        return Err(format!("view has more than {MAX_NODES} nodes"));
    }
    let object = node
        .as_object()
        .ok_or_else(|| "every node must be a JSON object".to_string())?;
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| "every node needs a string kind".to_string())?;
    if !UI_NODE_KINDS.contains(&kind) {
        return Err(format!(
            "unknown node kind {kind}; allowed: {}",
            UI_NODE_KINDS.join(", ")
        ));
    }
    if let Some(props) = object.get("props") {
        let props = props
            .as_object()
            .ok_or_else(|| format!("node {kind}: props must be an object"))?;
        if let Some(action) = props.get("action") {
            validate_action(action)?;
        }
    }
    if let Some(children) = object.get("children") {
        let children = children
            .as_array()
            .ok_or_else(|| format!("node {kind}: children must be an array"))?;
        for child in children {
            validate_node(child, depth + 1, nodes)?;
        }
    }
    Ok(())
}

/// Actions are the only way a view can affect the node. Keep the set small and
/// validate URLs here so the shell can pass them to the opener unguarded.
fn validate_action(action: &Value) -> Result<(), String> {
    let object = action
        .as_object()
        .ok_or_else(|| "action must be an object".to_string())?;
    let kind = object
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| "action.kind is required".to_string())?;
    match kind {
        "message" => {
            if !object.get("text").and_then(Value::as_str).is_some() {
                return Err("action.text must be a string".to_string());
            }
            Ok(())
        }
        "open_url" => {
            let url = object
                .get("url")
                .and_then(Value::as_str)
                .ok_or_else(|| "action.url must be a string".to_string())?;
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err("action.url must start with http:// or https://".to_string());
            }
            Ok(())
        }
        other => Err(format!("unknown action kind {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({
            "version": 1,
            "title": "Task",
            "root": {
                "kind": "stack",
                "props": { "gap": "md" },
                "children": [
                    { "kind": "text", "props": { "text": "hello" } },
                    { "kind": "button", "props": {
                        "label": "Inspect",
                        "action": { "kind": "message", "text": "inspect" }
                    }}
                ]
            }
        })
    }

    #[test]
    fn valid_view_is_accepted() {
        assert!(validate_view(&sample()).is_ok());
    }

    #[test]
    fn unknown_kind_is_rejected() {
        let mut view = sample();
        view["root"]["children"][0]["kind"] = json!("iframe");
        let error = validate_view(&view).unwrap_err();
        assert!(error.contains("unknown node kind"));
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let mut view = sample();
        view["version"] = json!(99);
        assert!(validate_view(&view).is_err());
    }

    #[test]
    fn deep_nesting_is_rejected() {
        let mut node = json!({ "kind": "text", "props": {} });
        for _ in 0..(MAX_DEPTH + 2) {
            node = json!({ "kind": "stack", "children": [node] });
        }
        let view = json!({ "version": 1, "root": node });
        assert!(validate_view(&view).is_err());
    }

    #[test]
    fn non_http_action_url_is_rejected() {
        let view = json!({
            "version": 1,
            "root": {
                "kind": "button",
                "props": {
                    "label": "x",
                    "action": { "kind": "open_url", "url": "file:///etc/passwd" }
                }
            }
        });
        assert!(validate_view(&view).is_err());
    }
}
