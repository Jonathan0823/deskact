//! Queries over the accessibility snapshot JSON returned by `agent-desktop snapshot`.

use serde_json::Value;

use crate::DeskActError;

/// Iterate every node of a snapshot's `tree` depth-first, parents before children.
pub(crate) fn nodes(snapshot: &Value) -> impl Iterator<Item = &Value> {
    let mut pending: Vec<&Value> = snapshot.get("tree").into_iter().collect();
    std::iter::from_fn(move || {
        let node = pending.pop()?;
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            pending.extend(children.iter().rev());
        }
        Some(node)
    })
}

/// Return a node's accessibility role.
pub(crate) fn role(node: &Value) -> Option<&str> {
    node.get("role").and_then(Value::as_str)
}

/// Return a node's accessible name.
pub(crate) fn name(node: &Value) -> Option<&str> {
    node.get("name").and_then(Value::as_str)
}

/// Report whether a node lists `state` among its accessibility states.
pub(crate) fn has_state(node: &Value, state: &str) -> bool {
    node.get("states")
        .and_then(Value::as_array)
        .is_some_and(|states| {
            states
                .iter()
                .any(|candidate| candidate.as_str() == Some(state))
        })
}

/// Iterate actionable nodes (those with a fresh ref) that exactly match a role and optional name.
pub(crate) fn controls<'a>(
    snapshot: &'a Value,
    role_filter: &str,
    name_filter: Option<&str>,
) -> impl Iterator<Item = &'a Value> {
    nodes(snapshot).filter(move |node| {
        role(node) == Some(role_filter)
            && name_filter.is_none_or(|expected| name(node) == Some(expected))
            && node.get("ref_id").and_then(Value::as_str).is_some()
    })
}

/// Find the single fresh actionable control matching the requested role and name.
pub(crate) fn unique_control<'a>(
    snapshot: &'a Value,
    role: &str,
    name: Option<&str>,
    description: &str,
) -> Result<&'a Value, DeskActError> {
    let matches: Vec<_> = controls(snapshot, role, name).collect();
    match matches.as_slice() {
        [control] => Ok(*control),
        _ => Err(DeskActError(format!(
            "Expected one unique {description}; found {}",
            matches.len()
        ))),
    }
}

/// Find the single fresh control with the requested role, name, and focused state.
pub(crate) fn unique_focused_control<'a>(
    snapshot: &'a Value,
    role: &str,
    name: &str,
    description: &str,
) -> Result<&'a Value, DeskActError> {
    let matches: Vec<_> = controls(snapshot, role, Some(name))
        .filter(|node| has_state(node, "focused"))
        .collect();
    match matches.as_slice() {
        [control] => Ok(*control),
        _ => Err(DeskActError(format!(
            "Expected one focused {description}; found {}",
            matches.len()
        ))),
    }
}

/// Return a control's fresh ref, which later native commands use to address it.
pub(crate) fn control_ref(control: &Value) -> Result<&str, DeskActError> {
    control
        .get("ref_id")
        .and_then(Value::as_str)
        .ok_or_else(|| DeskActError("Accessibility control had no fresh ref".into()))
}

/// Check whether any node in the snapshot exposes exactly the expected value.
pub(crate) fn contains_value(snapshot: &Value, expected: &str) -> bool {
    nodes(snapshot).any(|node| node.get("value").and_then(Value::as_str) == Some(expected))
}

/// Collect text for nodes whose native automation ID matches: the name after `prefix`, else the value.
pub(crate) fn texts_by_automation_id(
    snapshot: &Value,
    automation_id: &str,
    prefix: &str,
) -> Vec<String> {
    nodes(snapshot)
        .filter(|node| {
            node.pointer("/native_id/value").and_then(Value::as_str) == Some(automation_id)
        })
        .filter_map(|node| {
            name(node)
                .and_then(|name| name.strip_prefix(prefix))
                .or_else(|| node.get("value").and_then(Value::as_str))
        })
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn fixture() -> Value {
        json!({
            "tree": {
                "role": "window",
                "name": "Fixture",
                "children": [
                    {"role": "button", "name": "Save", "ref_id": "@e1", "states": ["focused"]},
                    {"role": "group", "children": [
                        {"role": "button", "name": "Save", "ref_id": "@e2", "states": ["disabled"]},
                        {"role": "button", "name": "Cancel"},
                        {"role": "textfield", "name": "Address Bar", "ref_id": "@e3",
                         "value": "C:\\work", "states": ["focused"]}
                    ]},
                    {"role": "statictext", "name": "Display is 42",
                     "native_id": {"value": "CalculatorResults"}},
                    {"role": "statictext", "name": "ignored", "value": "typed",
                     "native_id": {"value": "CalculatorExpression"}}
                ]
            }
        })
    }

    /// Nodes are visited parents first, in document order.
    #[test]
    fn nodes_are_depth_first_in_document_order() {
        let snapshot = fixture();
        let names: Vec<_> = nodes(&snapshot).filter_map(name).collect();

        assert_eq!(
            names,
            [
                "Fixture",
                "Save",
                "Save",
                "Cancel",
                "Address Bar",
                "Display is 42",
                "ignored"
            ]
        );
        assert_eq!(nodes(&json!({})).count(), 0);
    }

    /// Controls must match role and exact name and carry a ref.
    #[test]
    fn controls_filter_by_role_name_and_ref() {
        let snapshot = fixture();

        assert_eq!(controls(&snapshot, "button", Some("Save")).count(), 2);
        assert_eq!(controls(&snapshot, "button", None).count(), 2);
        assert_eq!(controls(&snapshot, "button", Some("Cancel")).count(), 0);
        assert_eq!(controls(&snapshot, "button", Some("save")).count(), 0);
    }

    /// Unique lookups report the match count in their existing diagnostics.
    #[test]
    fn unique_lookups_require_exactly_one_match() {
        let snapshot = fixture();

        let control = unique_control(&snapshot, "textfield", None, "editor").expect("one");
        assert_eq!(control_ref(control).expect("ref"), "@e3");
        assert_eq!(
            unique_control(&snapshot, "button", Some("Save"), "Save button")
                .expect_err("two")
                .to_string(),
            "Expected one unique Save button; found 2"
        );
        let focused = unique_focused_control(&snapshot, "button", "Save", "Save").expect("one");
        assert_eq!(control_ref(focused).expect("ref"), "@e1");
        assert_eq!(
            unique_focused_control(&snapshot, "button", "Cancel", "Cancel")
                .expect_err("none")
                .to_string(),
            "Expected one focused Cancel; found 0"
        );
        assert!(control_ref(&json!({"role": "button"})).is_err());
    }

    /// Disabled-state checks read the node's explicit states only.
    #[test]
    fn has_state_reads_explicit_states() {
        let snapshot = fixture();
        let disabled = controls(&snapshot, "button", Some("Save"))
            .nth(1)
            .expect("second");

        assert!(has_state(disabled, "disabled"));
        assert!(!has_state(disabled, "focused"));
        assert!(!has_state(&json!({"role": "button"}), "disabled"));
    }

    /// Value search is exact and covers nested nodes.
    #[test]
    fn contains_value_matches_exact_nested_values() {
        let snapshot = fixture();

        assert!(contains_value(&snapshot, "C:\\work"));
        assert!(!contains_value(&snapshot, "C:"));
        assert!(!contains_value(&json!({}), "typed"));
    }

    /// Automation-ID text prefers the prefixed name and falls back to the value.
    #[test]
    fn automation_id_text_uses_prefixed_name_then_value() {
        let snapshot = fixture();

        assert_eq!(
            texts_by_automation_id(&snapshot, "CalculatorResults", "Display is "),
            ["42"]
        );
        assert_eq!(
            texts_by_automation_id(&snapshot, "CalculatorExpression", "Expression is "),
            ["typed"]
        );
        assert!(texts_by_automation_id(&snapshot, "Missing", "").is_empty());
    }
}
