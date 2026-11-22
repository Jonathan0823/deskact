//! Agent-task orchestration for the dev harness: routing, console prompts, progress output and
//! outcome reporting that separates Rust-verified outcomes from model-reported ones (ROUTE-06).
//!
//! The model's `finish` summary is never evidence. A task is verified only when every
//! caller-supplied `Expectation` is confirmed by Rust from a fresh readback or the file on disk.

use std::{
    fs,
    io::{BufRead, Write},
    path::PathBuf,
};

use serde_json::Value;

use crate::{
    AgentBudget, AgentDesktop, AgentLoopReport, AgentModel, AgentOutcome, AgentStep,
    AppCatalogEntry, CancellationToken, DeskActError, DesktopCall, DesktopObserver, Observation,
    ObservedDesktop, StepApprover, UserAnswers,
    run_agent_loop_with_answers, step_policy::escape_text, validate_open_app_request,
};

/// Which path a request takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskRoute {
    /// An explicit `open <app>` request: the existing JSON action-plan fast path.
    OpenApp,
    /// Anything else: the observation-driven agent loop.
    AgentLoop,
}

/// Route a request: open-app requests keep the fast path (ROUTE-06), all else uses the loop.
pub fn route_request(request: &str, catalog: &[AppCatalogEntry]) -> TaskRoute {
    if validate_open_app_request(request, catalog).is_ok() {
        TaskRoute::OpenApp
    } else {
        TaskRoute::AgentLoop
    }
}

/// An outcome Rust can confirm independently of the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expectation {
    /// The target window's single editor holds exactly this text (Notepad readback).
    EditorText(String),
    /// The file at `path` holds exactly `contents` when read from disk.
    SavedFile {
        /// File that must exist.
        path: PathBuf,
        /// Exact expected contents.
        contents: String,
    },
}

/// Result of checking one expectation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verification {
    /// Rust confirmed the outcome; the text is the evidence.
    Verified(String),
    /// Rust could not confirm the outcome; the text says why.
    Unverified(String),
}

/// Final result of an agent task: the loop report plus Rust's verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTaskReport {
    /// Why and how the loop stopped.
    pub run: AgentLoopReport,
    /// One entry per expectation; empty unless the model reported `finish`.
    pub verification: Vec<Verification>,
}

impl AgentTaskReport {
    /// True only when the model finished and every expectation was confirmed by Rust.
    pub fn is_verified(&self) -> bool {
        matches!(self.run.outcome, AgentOutcome::Finished { .. })
            && !self.verification.is_empty()
            && self
                .verification
                .iter()
                .all(|item| matches!(item, Verification::Verified(_)))
    }

    /// Human-readable final report for the console.
    pub fn render(&self) -> String {
        let mut lines = Vec::new();
        match &self.run.outcome {
            AgentOutcome::Finished { summary } if self.is_verified() => {
                for item in &self.verification {
                    if let Verification::Verified(evidence) = item {
                        lines.push(format!("Verified outcome: {evidence}"));
                    }
                }
                lines.push(format!("Model summary (not itself verified): {summary}"));
            }
            AgentOutcome::Finished { summary } => {
                lines.push(format!("Unverified (model-reported): {summary}"));
                for item in &self.verification {
                    match item {
                        Verification::Unverified(reason) => {
                            lines.push(format!("Could not verify: {reason}"));
                        }
                        Verification::Verified(evidence) => {
                            lines.push(format!("Confirmed: {evidence}"));
                        }
                    }
                }
            }
            AgentOutcome::AskedUser { question } => {
                lines.push(format!(
                    "Stopped: the assistant asked \"{question}\" and got no answer."
                ));
            }
            AgentOutcome::StoppedByUser => lines.push("Stopped by user.".into()),
            AgentOutcome::ModelError(error) => {
                lines.push(format!("Partial and unverified: model error ({error})."));
            }
            AgentOutcome::Cancelled => lines.push("Cancelled.".into()),
            AgentOutcome::BudgetExhausted(limit) => {
                lines.push(format!(
                    "Partial and unverified: budget exhausted ({limit:?})."
                ));
            }
        }
        lines.push(format!(
            "Model turns: {}; tool calls: {}.",
            self.run.model_turns, self.run.tool_calls
        ));
        lines.join("\n")
    }
}

/// Run the agent loop, then verify each expectation if (and only if) the model reported `finish`.
#[allow(clippy::too_many_arguments)]
pub async fn run_agent_task(
    request: &str,
    model: &mut impl AgentModel,
    desktop: &mut impl AgentDesktop,
    user: &mut (impl StepApprover + UserAnswers),
    cancellation: &CancellationToken,
    budget: AgentBudget,
    expectations: &[Expectation],
) -> Result<AgentTaskReport, DeskActError> {
    let run =
        run_agent_loop_with_answers(request, model, desktop, user, cancellation, budget).await?;
    let verification = if matches!(run.outcome, AgentOutcome::Finished { .. }) {
        expectations
            .iter()
            .map(|expectation| verify(expectation, desktop))
            .collect()
    } else {
        Vec::new()
    };
    Ok(AgentTaskReport { run, verification })
}

fn verify(expectation: &Expectation, desktop: &mut impl AgentDesktop) -> Verification {
    match expectation {
        Expectation::EditorText(expected) => match verify_editor_text(expected, desktop) {
            Ok(evidence) => Verification::Verified(evidence),
            Err(reason) => Verification::Unverified(reason),
        },
        Expectation::SavedFile { path, contents } => match fs::read_to_string(path) {
            Ok(actual) if actual == *contents => Verification::Verified(format!(
                "saved file {} read from disk matches exactly",
                path.display()
            )),
            Ok(_) => Verification::Unverified(format!(
                "saved file {} does not contain the expected contents",
                path.display()
            )),
            Err(error) => Verification::Unverified(format!(
                "saved file {} could not be read: {error}",
                path.display()
            )),
        },
    }
}

fn verify_editor_text(expected: &str, desktop: &mut impl AgentDesktop) -> Result<String, String> {
    let Observation { text, truncated } = desktop
        .observe(None)
        .map_err(|error| format!("fresh observation failed: {error}"))?;
    if truncated {
        return Err("the fresh observation was truncated, so the editor is not unique".into());
    }
    let tree: Value = serde_json::from_str(&text)
        .map_err(|_| "the fresh observation was not readable".to_owned())?;
    let mut editors = Vec::new();
    collect_editors(
        tree.get("tree").unwrap_or(&Value::Null),
        false,
        &mut editors,
    );
    let [editor] = editors.as_slice() else {
        return Err(format!(
            "expected exactly one editor in the window, found {}",
            editors.len()
        ));
    };
    let actual = desktop
        .read_value(editor)
        .map_err(|error| format!("editor readback failed: {error}"))?;
    if actual == expected {
        Ok("editor text read back from the window matches exactly".into())
    } else {
        Err("editor text read back from the window does not match".into())
    }
}

/// Collect refs of text fields outside dialogs.
fn collect_editors(node: &Value, in_dialog: bool, found: &mut Vec<String>) {
    let role = node.get("role").and_then(Value::as_str).unwrap_or_default();
    if !in_dialog
        && role == "textfield"
        && let Some(reference) = node.get("ref_id").and_then(Value::as_str)
    {
        found.push(reference.to_owned());
    }
    for child in node
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        collect_editors(child, in_dialog || role == "dialog", found);
    }
}

/// Approval and question prompts over any reader and writer; the harness uses stdin and stderr.
pub struct ConsolePrompts<R, W> {
    input: R,
    output: W,
}

impl<R: BufRead, W: Write> ConsolePrompts<R, W> {
    /// Prompt on `output` and read replies from `input`.
    pub fn new(input: R, output: W) -> Self {
        Self { input, output }
    }

    fn ask(&mut self, prompt: &str) -> Option<String> {
        write!(self.output, "{prompt}").ok()?;
        self.output.flush().ok()?;
        let mut line = String::new();
        match self.input.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim().to_owned()),
        }
    }
}

/// Longest typed text shown in full in an approval prompt.
const PROMPT_TEXT_LIMIT: usize = 300;

/// Describe the exact scope of a step for approval: names, roles, text, path and keys, all escaped
/// so on-screen or model text cannot forge prompt lines or terminal escapes.
pub(crate) fn describe_step(step: &AgentStep) -> String {
    match step {
        AgentStep::Click {
            element_name,
            element_role,
            in_confirmation_dialog,
        } => {
            let place = if *in_confirmation_dialog {
                " in a confirmation dialog"
            } else {
                ""
            };
            format!(
                "click \"{}\" ({}){place}",
                escape_text(element_name),
                escape_text(element_role)
            )
        }
        AgentStep::Press {
            keys,
            focused_element_name,
            ..
        } => {
            let focus = match focused_element_name {
                Some(name) => format!("focus on \"{}\"", escape_text(name)),
                None => "focus unknown".to_owned(),
            };
            format!("press {} ({focus})", escape_text(keys))
        }
        AgentStep::SetValue {
            element_name,
            element_role,
            new_text,
            ..
        } => {
            let shown: String = new_text.chars().take(PROMPT_TEXT_LIMIT).collect();
            let total = new_text.chars().count();
            let more = if total > PROMPT_TEXT_LIMIT {
                format!("... ({total} characters in all)")
            } else {
                String::new()
            };
            format!(
                "replace the value of \"{}\" ({}) with \"{}{more}\"",
                escape_text(element_name),
                escape_text(element_role),
                escape_text(&shown)
            )
        }
        AgentStep::SaveAs { path, .. } => match path {
            Some(path) => format!("confirm Save As to \"{}\"", escape_text(path)),
            None => "confirm Save As to an unresolved path".to_owned(),
        },
        AgentStep::Unclassifiable { reason } => {
            format!("unclassified step ({})", escape_text(reason))
        }
    }
}

impl<R: BufRead, W: Write> StepApprover for ConsolePrompts<R, W> {
    fn approve(&mut self, step: &AgentStep, reason: &str) -> bool {
        let prompt = format!(
            "Approval needed: {} ({reason}).\nApprove this step? [y/N]: ",
            describe_step(step)
        );
        self.ask(&prompt)
            .is_some_and(|reply| matches!(reply.to_lowercase().as_str(), "y" | "yes"))
    }
}

impl<R: BufRead, W: Write> UserAnswers for ConsolePrompts<R, W> {
    fn answer(&mut self, question: &str) -> Option<String> {
        self.ask(&format!(
            "The assistant asks: {question}\nYour answer (empty to stop the task): "
        ))
        .filter(|reply| !reply.is_empty())
    }
}

/// Prints one progress line per executed action.
pub struct ProgressLog<W> {
    output: W,
}

impl<W: Write> DesktopObserver for ProgressLog<W> {
    fn before(&mut self, call: DesktopCall<'_>) {
        let line = match call {
            DesktopCall::Observe(root) => format!("observe {}", root.unwrap_or("window")),
            DesktopCall::Click(element) => format!("click {element}"),
            DesktopCall::SetValue(element, text) => {
                format!("set_value {element} ({} chars)", text.chars().count())
            }
            DesktopCall::ReadValue(element) => format!("read_value {element}"),
            DesktopCall::Press(keys) => format!("press {}", escape_text(keys)),
        };
        let _ = writeln!(self.output, "  -> {line}");
    }
}

/// Prints one progress line per executed action, then delegates to the wrapped desktop.
pub type ReportingDesktop<D, W> = ObservedDesktop<D, ProgressLog<W>>;

impl<D, W: Write> ObservedDesktop<D, ProgressLog<W>> {
    /// Wrap `inner`, writing progress to `output`.
    pub fn new(inner: D, output: W) -> Self {
        Self::wrap(inner, ProgressLog { output })
    }
}
