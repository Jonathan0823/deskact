//! Policy-bounded execution of validated model action plans.

use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

use crate::{
    ActionPlan, ActionPlanStep, AppCatalogEntry, DeskActError, NativeTargetResolution,
    prepare_app_selection, validate_action_plan_json,
};

/// Public result from one completed task-execution request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskExecutionReport {
    /// Deterministic workflow or primary-model route.
    pub route: String,
    /// Whether the task reached its locally verified postcondition.
    pub status: String,
    /// Validated plan summary.
    pub summary: String,
    /// Postconditions supported by fresh native evidence.
    pub achieved_postconditions: Vec<String>,
    /// Steps whose outcomes could not be verified or that were not attempted.
    pub unresolved_steps: Vec<String>,
    /// Evidence for each native resolution.
    pub evidence: Vec<TaskExecutionEvidence>,
}

/// Native evidence recorded for one supported app-opening step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskExecutionEvidence {
    /// Locally selected catalog ID.
    pub app_id: String,
    /// Canonical catalog name.
    pub app_name: String,
    /// Fresh native window identity.
    pub window_id: String,
    /// Fresh complete accessibility snapshot identity.
    pub snapshot_id: String,
    /// Number of actionable references in the snapshot.
    pub ref_count: usize,
}

/// Build a deterministic, registered `open <canonical app name>` workflow when uniquely matched.
pub fn registered_open_workflow(
    request: &str,
    catalog: &[AppCatalogEntry],
) -> Result<Option<(ActionPlan, AppCatalogEntry)>, DeskActError> {
    if catalog.is_empty() {
        return Ok(None);
    }
    let Some((verb, app_request)) = request.trim().split_once(' ') else {
        return Ok(None);
    };
    if !verb.eq_ignore_ascii_case("open") {
        return Ok(None);
    }
    let app_request = app_request.trim();
    if app_request.is_empty() {
        return Ok(None);
    }
    match prepare_app_selection(catalog, app_request)? {
        crate::AppSelectionInput::ExactMatch { entry } => {
            let plan = open_app_plan(&entry);
            Ok(Some((plan, entry)))
        }
        crate::AppSelectionInput::NeedsClarification { .. } => Err(DeskActError::new(
            "More than one app exactly matches this request; ask the user to choose. No app was launched.".into(),
        )),
        crate::AppSelectionInput::NeedsModel { .. } => Ok(None),
    }
}

/// Create the local plan used by the deterministic open-app workflow.
pub fn open_app_plan(entry: &AppCatalogEntry) -> ActionPlan {
    let mut parameters = BTreeMap::new();
    parameters.insert("app_id".to_owned(), json!(entry.id));
    ActionPlan {
        version: 1,
        summary: format!("Open {}", entry.name),
        steps: vec![ActionPlanStep {
            operation: "open_app".into(),
            parameters,
            expected_outcome: format!("{} is open", entry.name),
        }],
    }
}

/// Prevalidate the entire plan, then execute its supported app-opening steps through a resolver.
pub fn execute_action_plan(
    plan: &ActionPlan,
    catalog: &[AppCatalogEntry],
    route: &str,
    mut resolve: impl FnMut(&AppCatalogEntry) -> Result<NativeTargetResolution, DeskActError>,
) -> Result<TaskExecutionReport, DeskActError> {
    let serialized_plan = serde_json::to_string(plan)
        .map_err(|_| DeskActError::new("Could not revalidate the action plan".into()))?;
    let plan = validate_action_plan_json(&serialized_plan)?;
    let mut selected = Vec::with_capacity(plan.steps.len());
    for (index, step) in plan.steps.iter().enumerate() {
        if step.operation != "open_app" {
            return Err(DeskActError::new(format!(
                "Action plan step {} uses unsupported operation `{}`; no actions were executed",
                index + 1,
                step.operation
            )));
        }
        if step.parameters.len() != 1 || !step.parameters.contains_key("app_id") {
            return Err(DeskActError::new(format!(
                "Action plan step {} must contain exactly one `app_id` parameter; no actions were executed",
                index + 1
            )));
        }
        let app_id = step.parameters["app_id"].as_str().ok_or_else(|| {
            DeskActError::new(format!(
                "Action plan step {} `app_id` must be a string; no actions were executed",
                index + 1
            ))
        })?;
        let app = catalog
            .iter()
            .find(|entry| entry.id == app_id)
            .cloned()
            .ok_or_else(|| {
                DeskActError::new(format!(
                    "Action plan step {} selected an app ID outside the local catalog; no actions were executed",
                    index + 1
                ))
            })?;
        let verified_expectation = format!("{} is open", app.name);
        if !step
            .expected_outcome
            .eq_ignore_ascii_case(&verified_expectation)
        {
            return Err(DeskActError::new(format!(
                "Action plan step {} requested an unsupported postcondition; no actions were executed",
                index + 1
            )));
        }
        selected.push(app);
    }

    let mut evidence = Vec::with_capacity(selected.len());
    let mut unresolved_steps = Vec::new();
    for (index, app) in selected.iter().enumerate() {
        let resolution = match resolve(app) {
            Ok(resolution) => resolution,
            Err(error) => {
                unresolved_steps.push(format!("Step {} (open {}): {error}", index + 1, app.name));
                break;
            }
        };
        let resolved = match resolution {
            NativeTargetResolution::Ready(target) => target,
            NativeTargetResolution::NeedsManualRestore { window, .. } => {
                unresolved_steps.push(format!(
                    "Step {} (open {}): window `{}` is minimized and needs manual restoration",
                    index + 1,
                    app.name,
                    window.title
                ));
                break;
            }
        };
        if !resolved.window.accessible || resolved.window.minimized || resolved.ref_count == 0 {
            unresolved_steps.push(format!(
                "Step {} (open {}): resolved window `{}` did not satisfy the accessible, non-minimized snapshot precondition",
                index + 1,
                app.name,
                resolved.window.id
            ));
            break;
        }
        evidence.push(TaskExecutionEvidence {
            app_id: app.id.clone(),
            app_name: app.name.clone(),
            window_id: resolved.window.id,
            snapshot_id: resolved.snapshot_id,
            ref_count: resolved.ref_count,
        });
    }
    for (index, app) in selected
        .iter()
        .enumerate()
        .skip(evidence.len() + unresolved_steps.len())
    {
        unresolved_steps.push(format!(
            "Step {} (open {}): not attempted after an earlier step could not be verified",
            index + 1,
            app.name
        ));
    }

    let achieved_postconditions = evidence
        .iter()
        .map(|evidence| {
            format!(
                "{} is open in one uniquely matched accessible window with a fresh complete snapshot",
                evidence.app_name
            )
        })
        .collect();
    let summary = if evidence.is_empty() {
        "No app window was verified".to_owned()
    } else {
        format!(
            "Opened {}",
            evidence
                .iter()
                .map(|evidence| evidence.app_name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    Ok(TaskExecutionReport {
        route: route.to_owned(),
        status: if unresolved_steps.is_empty() {
            "completed".into()
        } else if evidence.is_empty() {
            "failed".into()
        } else {
            "partial".into()
        },
        summary,
        achieved_postconditions,
        unresolved_steps,
        evidence,
    })
}

/// Prepare the catalog-only model context; executable paths and launch arguments stay local.
pub fn model_candidate_context(catalog: &[AppCatalogEntry]) -> Result<String, DeskActError> {
    let candidates: Vec<_> = catalog
        .iter()
        .map(|entry| json!({ "id": entry.id, "display_name": entry.name }))
        .collect();
    serde_json::to_string(&Value::Array(candidates))
        .map_err(|_| DeskActError::new("Could not serialize app candidates for the model".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppCatalogSource;

    #[test]
    fn native_failure_is_reported_as_an_unresolved_step() {
        let entry = AppCatalogEntry {
            id: "calculator-id".into(),
            name: "Calculator".into(),
            target: "C:\\Windows\\System32\\calc.exe".into(),
            arguments: String::new(),
            working_directory: String::new(),
            source: AppCatalogSource::UserConfigured,
            aliases: Vec::new(),
        };

        let report = execute_action_plan(&open_app_plan(&entry), &[entry], "primary_model", |_| {
            Err(DeskActError::new("fixture resolution failure".into()))
        })
        .expect("unverified execution should produce a report");

        assert_eq!(report.status, "failed");
        assert!(report.achieved_postconditions.is_empty());
        assert_eq!(report.unresolved_steps.len(), 1);
        assert!(report.unresolved_steps[0].contains("fixture resolution failure"));
    }
}
