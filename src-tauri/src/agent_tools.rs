//! Fixed agent-loop tool set confined to one verified target window (ADR 0002, ROUTE-06).
//!
//! The model proposes calls; these tools only validate scope and execute through `agent-desktop`.
//! Classification and authorization (POL-01a) belong to the caller and run before these tools.

use std::collections::HashSet;

use serde_json::{Map, Value, json};

use crate::{DeskActError, NATIVE_PROCESS_TIMEOUT, NativeWindow, VerifiedNativeCli, snapshot};

/// Largest observation, in bytes, returned to the model.
pub const OBSERVATION_BYTE_CAP: usize = 16 * 1024;
/// Longest value or name text kept per node; use `read_value` for full contents.
const NODE_TEXT_LIMIT: usize = 200;

/// A compact, size-capped view of the target window for the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    /// Compact JSON of interactive controls, including dialogs the window owns.
    pub text: String,
    /// Whether controls were dropped to respect `OBSERVATION_BYTE_CAP`.
    pub truncated: bool,
}

/// The `observe`, `click`, `set_value`, `press` and `read_value` tools for one target window.
///
/// Only refs that appeared in an observation of this window are addressable, so refs from other
/// windows (including other windows of the same app) are rejected before reaching the CLI.
pub struct TargetWindowTools {
    cli: VerifiedNativeCli,
    window: NativeWindow,
    observed_refs: HashSet<String>,
}

impl TargetWindowTools {
    /// Confine a verified native CLI to one resolved target window.
    pub fn new(cli: VerifiedNativeCli, window: NativeWindow) -> Self {
        Self {
            cli,
            window,
            observed_refs: HashSet::new(),
        }
    }

    /// Snapshot the target window, or drill down to an already observed ref.
    pub fn observe(&mut self, root: Option<&str>) -> Result<Observation, DeskActError> {
        if let Some(root) = root {
            self.require_observed(root)?;
        }
        let data = self
            .cli
            .fresh_snapshot_scoped(&self.window, root, NATIVE_PROCESS_TIMEOUT)?;
        let tree = data.get("tree").unwrap_or(&Value::Null);
        let (text, truncated, refs) = compact_within_cap(&self.window.title, tree);
        if root.is_none() {
            self.observed_refs.clear();
        }
        self.observed_refs.extend(refs);
        Ok(Observation { text, truncated })
    }

    /// Click an observed control.
    pub fn click(&mut self, element: &str) -> Result<(), DeskActError> {
        self.require_observed(element)?;
        self.cli.invoke_success(&["click", element], "click")?;
        Ok(())
    }

    /// Replace the value of an observed control.
    pub fn set_value(&mut self, element: &str, text: &str) -> Result<(), DeskActError> {
        self.require_observed(element)?;
        self.cli
            .invoke_success(&["set-value", element, text], "set-value")?;
        Ok(())
    }

    /// Read the full value of an observed control.
    pub fn read_value(&self, element: &str) -> Result<String, DeskActError> {
        self.require_observed(element)?;
        let value = self
            .cli
            .invoke_value(&["get", element, "--property", "value"], "get")?;
        Ok(match value {
            Value::String(text) => text,
            other => other.to_string(),
        })
    }

    /// Press keys in the target window after focus on it is confirmed; sends nothing otherwise.
    pub fn press(&self, keys: &str) -> Result<(), DeskActError> {
        if keys.trim().is_empty() || keys.starts_with('-') || keys.chars().any(char::is_control) {
            return Err(DeskActError(format!(
                "Refusing malformed key sequence `{keys}`"
            )));
        }
        self.cli
            .press_app_shortcut(&self.window, &self.window.app_name, keys)
    }

    fn require_observed(&self, element: &str) -> Result<(), DeskActError> {
        if self.observed_refs.contains(element) {
            Ok(())
        } else {
            Err(DeskActError(format!(
                "Ref `{element}` is not a control of the target window `{}`; observe first",
                self.window.title
            )))
        }
    }
}

/// Keep the largest document-order prefix of the tree whose compact JSON fits the byte cap.
fn compact_within_cap(title: &str, tree: &Value) -> (String, bool, Vec<String>) {
    let total = snapshot::nodes(&json!({ "tree": tree })).count();
    let render = |limit: usize| {
        let mut budget = limit;
        let kept = compact_node(tree, &mut budget);
        let truncated = limit < total;
        let text = json!({ "window": title, "truncated": truncated, "tree": kept }).to_string();
        (text, truncated, kept)
    };
    let (mut low, mut high) = (1, total.max(1));
    while low < high {
        let middle = (low + high).div_ceil(2);
        if render(middle).0.len() <= OBSERVATION_BYTE_CAP {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    let (text, truncated, kept) = render(low);
    let refs = snapshot::nodes(&json!({ "tree": kept }))
        .filter_map(|node| node.get("ref_id").and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    (text, truncated, refs)
}

/// Copy the model-relevant fields of a node and its children while node budget remains.
fn compact_node(node: &Value, budget: &mut usize) -> Value {
    if *budget == 0 {
        return Value::Null;
    }
    *budget -= 1;
    let mut compact = Map::new();
    for key in ["role", "name", "value"] {
        if let Some(text) = node.get(key).and_then(Value::as_str) {
            compact.insert(key.into(), Value::String(limit_text(text)));
        }
    }
    for key in ["ref_id", "states"] {
        if let Some(value) = node.get(key) {
            compact.insert(key.into(), value.clone());
        }
    }
    let children: Vec<Value> = node
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|child| compact_node(child, budget))
        .filter(|child| !child.is_null())
        .collect();
    if !children.is_empty() {
        compact.insert("children".into(), Value::Array(children));
    }
    Value::Object(compact)
}

fn limit_text(text: &str) -> String {
    match text.char_indices().nth(NODE_TEXT_LIMIT) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}
