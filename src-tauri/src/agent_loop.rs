//! Observation-driven agent loop (ADR 0002, ROUTE-06): observe, propose, gate, execute, observe.
//!
//! The model only proposes tool calls. Rust classifies each step from facts it gathered itself
//! (POL-01a), asks for approval when needed, and executes through the target-window tools. Window
//! content is framed as untrusted data and never changes policy. The loop stops on `finish`,
//! `ask_user`, denial, cancellation, or an exhausted budget; it never claims a verified outcome.

use std::{
    collections::HashMap,
    future::Future,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::{
    AgentStep, AgentTool, AgentTurn, CancellationToken, DeskActError, Observation, StepApprover,
    StepGate, TargetWindowTools, gate_step, request_agent_turn, step_policy::escape_text,
};

/// DeskAct-owned tool prompt, adapted from the `agent-desktop` 1.1.0 skill text.
pub const AGENT_TOOL_PROMPT: &str = include_str!("agent_prompt.md");

/// Maximum model turns per task.
pub const MAX_MODEL_TURNS: usize = 25;
/// Maximum executed tool calls per task, including observations.
pub const MAX_EXECUTED_TOOL_CALLS: usize = 40;
/// Maximum wall-clock time per task.
pub const MAX_TASK_DURATION: Duration = Duration::from_secs(180);

const UNTRUSTED_OPEN: &str = "<untrusted_window_content>";
const UNTRUSTED_CLOSE: &str = "</untrusted_window_content>";

/// Per-task limits; defaults follow PRD ROUTE-06 and are tunable after real runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentBudget {
    /// Model turns allowed.
    pub max_model_turns: usize,
    /// Executed tool calls allowed (`finish` and `ask_user` are not executed calls).
    pub max_tool_calls: usize,
    /// Wall-clock time allowed.
    pub max_duration: Duration,
}

impl Default for AgentBudget {
    fn default() -> Self {
        Self {
            max_model_turns: MAX_MODEL_TURNS,
            max_tool_calls: MAX_EXECUTED_TOOL_CALLS,
            max_duration: MAX_TASK_DURATION,
        }
    }
}

/// Which budget limit ended the task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetLimit {
    /// Model turn limit reached.
    ModelTurns,
    /// Executed tool call limit reached.
    ToolCalls,
    /// Wall-clock limit reached.
    WallClock,
}

/// Why the loop stopped. None of these is a verified outcome; verification belongs to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentOutcome {
    /// The model reported completion; unverified until Rust confirms it.
    Finished {
        /// Model-written summary.
        summary: String,
    },
    /// The model needs an answer from the user.
    AskedUser {
        /// Question to show the user.
        question: String,
    },
    /// The user denied a flagged step; the model was not told.
    StoppedByUser,
    /// The model could not be reached or answered with an error; the work so far is partial.
    ModelError(String),
    /// Cancelled between steps.
    Cancelled,
    /// A budget limit ran out: the work is partial and unverified.
    BudgetExhausted(BudgetLimit),
}

/// Result of one loop run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLoopReport {
    /// Why the loop stopped.
    pub outcome: AgentOutcome,
    /// Model turns used.
    pub model_turns: usize,
    /// Tool calls executed, including observations.
    pub tool_calls: usize,
}

/// Port to the primary model for one agent turn.
pub trait AgentModel {
    /// Send the conversation (Chat Completions messages) and return validated proposals.
    fn next_turn(
        &mut self,
        messages: &[Value],
    ) -> impl Future<Output = Result<AgentTurn, DeskActError>>;
}

/// Model backed by the configured Chat Completions endpoint with tool calling.
pub struct ChatCompletionsAgentModel;

impl AgentModel for ChatCompletionsAgentModel {
    async fn next_turn(&mut self, messages: &[Value]) -> Result<AgentTurn, DeskActError> {
        request_agent_turn(messages.to_vec()).await
    }
}

/// Port to the confined target-window tools; `TargetWindowTools` is the production adapter.
pub trait AgentDesktop {
    /// Snapshot the target window, optionally rooted at an observed ref.
    fn observe(&mut self, root: Option<&str>) -> Result<Observation, DeskActError>;
    /// Click an observed control.
    fn click(&mut self, element: &str) -> Result<(), DeskActError>;
    /// Replace an observed control's value.
    fn set_value(&mut self, element: &str, text: &str) -> Result<(), DeskActError>;
    /// Read an observed control's value.
    fn read_value(&mut self, element: &str) -> Result<String, DeskActError>;
    /// Press keys after focus on the target window is confirmed.
    fn press(&mut self, keys: &str) -> Result<(), DeskActError>;
}

impl AgentDesktop for TargetWindowTools {
    fn observe(&mut self, root: Option<&str>) -> Result<Observation, DeskActError> {
        TargetWindowTools::observe(self, root)
    }
    fn click(&mut self, element: &str) -> Result<(), DeskActError> {
        TargetWindowTools::click(self, element)
    }
    fn set_value(&mut self, element: &str, text: &str) -> Result<(), DeskActError> {
        TargetWindowTools::set_value(self, element, text)
    }
    fn read_value(&mut self, element: &str) -> Result<String, DeskActError> {
        TargetWindowTools::read_value(self, element)
    }
    fn press(&mut self, keys: &str) -> Result<(), DeskActError> {
        TargetWindowTools::press(self, keys)
    }
}

impl<D: AgentDesktop> AgentDesktop for &mut D {
    fn observe(&mut self, root: Option<&str>) -> Result<Observation, DeskActError> {
        (**self).observe(root)
    }
    fn click(&mut self, element: &str) -> Result<(), DeskActError> {
        (**self).click(element)
    }
    fn set_value(&mut self, element: &str, text: &str) -> Result<(), DeskActError> {
        (**self).set_value(element, text)
    }
    fn read_value(&mut self, element: &str) -> Result<String, DeskActError> {
        (**self).read_value(element)
    }
    fn press(&mut self, keys: &str) -> Result<(), DeskActError> {
        (**self).press(keys)
    }
}

/// One desktop call, as reported to a `DesktopObserver`.
#[derive(Debug, Clone, Copy)]
pub enum DesktopCall<'a> {
    /// `observe` with an optional drill-down root.
    Observe(Option<&'a str>),
    /// `click` on a ref.
    Click(&'a str),
    /// `set_value` on a ref with new text.
    SetValue(&'a str, &'a str),
    /// `read_value` on a ref.
    ReadValue(&'a str),
    /// `press` with keys.
    Press(&'a str),
}

/// Watches desktop calls without changing them; used for progress output and run records.
pub trait DesktopObserver {
    /// Called before the call is forwarded.
    fn before(&mut self, _call: DesktopCall<'_>) {}
    /// Called after the call; `value` is the text a `read_value` returned.
    fn after(&mut self, _call: DesktopCall<'_>, _succeeded: bool, _value: Option<&str>) {}
}

/// Decorator that forwards every call to `inner` and reports it to `observer`.
pub struct ObservedDesktop<D, O> {
    /// The wrapped desktop.
    pub inner: D,
    /// The observer that sees every call.
    pub observer: O,
}

impl<D, O> ObservedDesktop<D, O> {
    /// Wrap `inner` so `observer` sees its calls.
    pub fn wrap(inner: D, observer: O) -> Self {
        Self { inner, observer }
    }
}

impl<D: AgentDesktop, O: DesktopObserver> AgentDesktop for ObservedDesktop<D, O> {
    fn observe(&mut self, root: Option<&str>) -> Result<Observation, DeskActError> {
        let call = DesktopCall::Observe(root);
        self.observer.before(call);
        let result = self.inner.observe(root);
        self.observer.after(call, result.is_ok(), None);
        result
    }
    fn click(&mut self, element: &str) -> Result<(), DeskActError> {
        let call = DesktopCall::Click(element);
        self.observer.before(call);
        let result = self.inner.click(element);
        self.observer.after(call, result.is_ok(), None);
        result
    }
    fn set_value(&mut self, element: &str, text: &str) -> Result<(), DeskActError> {
        let call = DesktopCall::SetValue(element, text);
        self.observer.before(call);
        let result = self.inner.set_value(element, text);
        self.observer.after(call, result.is_ok(), None);
        result
    }
    fn read_value(&mut self, element: &str) -> Result<String, DeskActError> {
        let call = DesktopCall::ReadValue(element);
        self.observer.before(call);
        let result = self.inner.read_value(element);
        self.observer
            .after(call, result.is_ok(), result.as_deref().ok());
        result
    }
    fn press(&mut self, keys: &str) -> Result<(), DeskActError> {
        let call = DesktopCall::Press(keys);
        self.observer.before(call);
        let result = self.inner.press(keys);
        self.observer.after(call, result.is_ok(), None);
        result
    }
}

/// Source of user answers to `ask_user` questions (console in the dev harness, island later).
pub trait UserAnswers {
    /// Show the question; return the answer, or `None` when the user gives none (the run then ends).
    fn answer(&mut self, question: &str) -> Option<String>;
}

/// Adds "never answers" to an approver: `ask_user` then ends the run with `AgentOutcome::AskedUser`.
struct ApproveOnly<'a, A>(&'a mut A);

impl<A: StepApprover> StepApprover for ApproveOnly<'_, A> {
    fn approve(&mut self, step: &AgentStep, reason: &str) -> bool {
        self.0.approve(step, reason)
    }
}

impl<A> UserAnswers for ApproveOnly<'_, A> {
    fn answer(&mut self, _question: &str) -> Option<String> {
        None
    }
}

/// Wrap on-screen text so the model sees it as data; the closing tag cannot be forged from inside.
///
/// Any spelling of the closing tag (any case, with or without the final `>`) is broken up.
pub fn frame_untrusted(content: &str) -> String {
    let closing = &UNTRUSTED_CLOSE[..UNTRUSTED_CLOSE.len() - 1];
    let lowered = content.to_ascii_lowercase();
    let (mut safe, mut copied) = (String::with_capacity(content.len()), 0);
    for (start, _) in lowered.match_indices(closing) {
        safe.push_str(&content[copied..start]);
        safe.push_str("<\\/");
        safe.push_str(&content[start + 2..start + closing.len()]);
        copied = start + closing.len();
    }
    safe.push_str(&content[copied..]);
    format!("{UNTRUSTED_OPEN}\n{safe}\n{UNTRUSTED_CLOSE}")
}

/// Facts about an observed control, taken from the observation and never from model claims.
#[derive(Debug, Clone, Default)]
struct Element {
    name: String,
    role: String,
    value: String,
    focused: bool,
    dialog: Option<String>,
}

#[derive(Default)]
struct Elements(HashMap<String, Element>);

impl Elements {
    /// Record an observation. A whole-window one replaces all facts; a drill-down below `root`
    /// merges and keeps the dialog context of the element it expands.
    fn record(&mut self, observation: &Observation, root: Option<&str>) {
        let dialog = match root {
            None => {
                self.0.clear();
                None
            }
            Some(root) => self.get(root).and_then(|element| {
                if element.role == "dialog" {
                    Some(element.name.clone())
                } else {
                    element.dialog.clone()
                }
            }),
        };
        if let Ok(parsed) = serde_json::from_str::<Value>(&observation.text)
            && let Some(tree) = parsed.get("tree")
        {
            self.collect(tree, dialog.as_deref());
        }
    }

    /// Forget everything: after a mutating action every ref and fact is stale.
    fn invalidate(&mut self) {
        self.0.clear();
    }

    fn collect(&mut self, node: &Value, dialog: Option<&str>) {
        let text = |key: &str| node.get(key).and_then(Value::as_str).unwrap_or_default();
        let (name, role) = (text("name"), text("role"));
        if let Some(reference) = node.get("ref_id").and_then(Value::as_str) {
            let focused = node
                .get("states")
                .and_then(Value::as_array)
                .is_some_and(|states| states.iter().any(|state| state == "focused"));
            self.0.insert(
                reference.to_owned(),
                Element {
                    name: name.to_owned(),
                    role: role.to_owned(),
                    value: text("value").to_owned(),
                    focused,
                    dialog: dialog.map(str::to_owned),
                },
            );
        }
        let inner = if role == "dialog" { Some(name) } else { dialog };
        for child in node
            .get("children")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            self.collect(child, inner);
        }
    }

    fn get(&self, reference: &str) -> Option<&Element> {
        self.0.get(reference)
    }

    /// The focused element, only when exactly one node claims focus; ambiguity is not guessed at.
    fn sole_focus(&self) -> Option<&Element> {
        let mut focused = self.0.values().filter(|element| element.focused);
        let first = focused.next()?;
        focused.next().is_none().then_some(first)
    }

    /// The file name field of the same Save As dialog as `element`.
    fn save_as_field(&self, element: &Element) -> Option<&str> {
        self.0.iter().find_map(|(reference, candidate)| {
            (candidate.dialog == element.dialog
                && matches!(candidate.role.as_str(), "textfield" | "edit")
                && candidate.name.to_lowercase().contains("file name"))
            .then_some(reference.as_str())
        })
    }

    /// The folder a Save As dialog is showing, when its address element exposes an absolute path
    /// (as its value, or after the `Address:` prefix of its name).
    fn dialog_folder(&self, element: &Element) -> Option<PathBuf> {
        self.0
            .values()
            .filter(|candidate| candidate.dialog == element.dialog)
            .filter(|candidate| candidate.name.to_lowercase().starts_with("address"))
            .find_map(|candidate| {
                let named = candidate.name.split_once(':').map(|(_, rest)| rest.trim());
                [Some(candidate.value.trim()), named]
                    .into_iter()
                    .flatten()
                    .map(Path::new)
                    .find(|path| path.is_absolute())
                    .map(Path::to_path_buf)
            })
    }
}

/// The Save As file dialog itself. The overwrite prompt "Confirm Save As" is a confirmation dialog.
fn in_save_as(element: &Element) -> bool {
    element.dialog.as_deref().is_some_and(|name| {
        let name = name.to_lowercase();
        name.contains("save as") && !name.contains("confirm")
    })
}

fn in_confirmation_dialog(element: &Element) -> bool {
    element.dialog.is_some() && !in_save_as(element)
}

/// Resolve the path a Save As would write and check whether it exists.
///
/// An absolute name is used as is; a bare or relative name is resolved against the folder the
/// dialog shows. When no folder can be observed the path stays unresolved and the step is flagged
/// as unclassifiable (fail closed).
fn save_as_step(
    desktop: &mut impl AgentDesktop,
    elements: &Elements,
    in_dialog: &Element,
) -> AgentStep {
    let typed = elements
        .save_as_field(in_dialog)
        .and_then(|field| desktop.read_value(field).ok())
        .unwrap_or_default();
    let typed = Path::new(typed.trim());
    let resolved = if typed.as_os_str().is_empty() {
        None
    } else if typed.is_absolute() {
        Some(typed.to_path_buf())
    } else {
        elements
            .dialog_folder(in_dialog)
            .map(|folder| folder.join(typed))
    };
    AgentStep::SaveAs {
        path_exists: resolved.as_ref().map(|path| path.exists()),
        path: resolved.map(|path| path.display().to_string()),
    }
}

/// Build the step Rust classifies; `None` for read-only tools. Click and set_value refs must have
/// been checked against `Elements` already.
fn step_for(
    tool: &AgentTool,
    desktop: &mut impl AgentDesktop,
    elements: &Elements,
) -> Option<AgentStep> {
    match tool {
        AgentTool::Click { element_ref } => {
            let element = elements.get(element_ref)?;
            if in_save_as(element) && element.name.trim().eq_ignore_ascii_case("save") {
                return Some(save_as_step(desktop, elements, element));
            }
            Some(AgentStep::Click {
                element_name: element.name.clone(),
                element_role: element.role.clone(),
                in_confirmation_dialog: in_confirmation_dialog(element),
            })
        }
        AgentTool::SetValue { element_ref, text } => {
            let element = elements.get(element_ref)?;
            Some(AgentStep::SetValue {
                element_name: element.name.clone(),
                element_role: element.role.clone(),
                new_text: text.clone(),
                current_value: desktop.read_value(element_ref).ok(),
            })
        }
        AgentTool::Press { keys } => {
            let focused = elements.sole_focus();
            let is_enter = matches!(keys.trim().to_lowercase().as_str(), "enter" | "return");
            if is_enter
                && let Some(element) = focused.filter(|element| {
                    in_save_as(element)
                        && (element.name.trim().eq_ignore_ascii_case("save")
                            || matches!(element.role.as_str(), "textfield" | "edit"))
                })
            {
                return Some(save_as_step(desktop, elements, element));
            }
            Some(AgentStep::Press {
                keys: keys.clone(),
                focused_element_name: focused.map(|element| element.name.clone()),
                focused_in_confirmation_dialog: focused.is_some_and(in_confirmation_dialog),
            })
        }
        AgentTool::Observe { .. }
        | AgentTool::ReadValue { .. }
        | AgentTool::Finish { .. }
        | AgentTool::AskUser { .. } => None,
    }
}

fn exhausted(
    budget: &AgentBudget,
    turns: usize,
    calls: usize,
    started: Instant,
) -> Option<BudgetLimit> {
    if started.elapsed() >= budget.max_duration {
        Some(BudgetLimit::WallClock)
    } else if turns >= budget.max_model_turns {
        Some(BudgetLimit::ModelTurns)
    } else if calls >= budget.max_tool_calls {
        Some(BudgetLimit::ToolCalls)
    } else {
        None
    }
}

fn tool_message(id: &str, content: String) -> Value {
    json!({"role": "tool", "tool_call_id": id, "content": content})
}

/// Run the loop with no way to answer `ask_user`; see `run_agent_loop_with_answers`.
pub async fn run_agent_loop(
    request: &str,
    model: &mut impl AgentModel,
    desktop: &mut impl AgentDesktop,
    approver: &mut impl StepApprover,
    cancellation: &CancellationToken,
    budget: AgentBudget,
) -> Result<AgentLoopReport, DeskActError> {
    run_agent_loop_with_answers(
        request,
        model,
        desktop,
        &mut ApproveOnly(approver),
        cancellation,
        budget,
    )
    .await
}

/// Run the loop for one request against one verified target window.
///
/// An answered `ask_user` continues the same conversation and budget; an unanswered one ends the run.
///
/// Starts with an observation (an executed tool call), then alternates model turns and gated tool
/// execution. Tool errors are returned to the model as results; model transport errors end the run.
pub async fn run_agent_loop_with_answers(
    request: &str,
    model: &mut impl AgentModel,
    desktop: &mut impl AgentDesktop,
    user: &mut (impl StepApprover + UserAnswers),
    cancellation: &CancellationToken,
    budget: AgentBudget,
) -> Result<AgentLoopReport, DeskActError> {
    let started = Instant::now();
    let (mut turns, mut calls) = (0, 0);
    let report = |outcome, turns, calls| AgentLoopReport {
        outcome,
        model_turns: turns,
        tool_calls: calls,
    };
    let mut elements = Elements::default();
    let mut messages = vec![json!({"role": "system", "content": AGENT_TOOL_PROMPT})];
    if cancellation.is_cancelled() {
        return Ok(report(AgentOutcome::Cancelled, turns, calls));
    }
    if let Some(limit) = exhausted(&budget, turns, calls, started) {
        return Ok(report(AgentOutcome::BudgetExhausted(limit), turns, calls));
    }
    let first = desktop.observe(None)?;
    calls += 1;
    elements.record(&first, None);
    messages.push(json!({"role": "user", "content": format!(
        "Task: {request}\n\nInitial observation of the target window:\n{}",
        frame_untrusted(&first.text)
    )}));

    loop {
        if cancellation.is_cancelled() {
            return Ok(report(AgentOutcome::Cancelled, turns, calls));
        }
        if let Some(limit) = exhausted(&budget, turns, calls, started) {
            return Ok(report(AgentOutcome::BudgetExhausted(limit), turns, calls));
        }
        turns += 1;
        let remaining = budget.max_duration.saturating_sub(started.elapsed());
        let turn = match tokio::time::timeout(remaining, model.next_turn(&messages)).await {
            Ok(Ok(turn)) => turn,
            Ok(Err(error)) => {
                return Ok(report(
                    AgentOutcome::ModelError(error.to_string()),
                    turns,
                    calls,
                ));
            }
            Err(_) => {
                let outcome = AgentOutcome::BudgetExhausted(BudgetLimit::WallClock);
                return Ok(report(outcome, turns, calls));
            }
        };
        messages.push(turn.assistant_message.clone());
        if turn.tool_calls.is_empty() {
            messages.push(json!({"role": "user", "content":
                "Respond with a tool call. Use finish when you believe the task is finished or ask_user if blocked."}));
            continue;
        }
        let mut asked = false;
        for call in turn.tool_calls {
            if cancellation.is_cancelled() {
                return Ok(report(AgentOutcome::Cancelled, turns, calls));
            }
            if asked {
                messages.push(tool_message(
                    &call.id,
                    "skipped: not run because the user was asked a question first".into(),
                ));
                continue;
            }
            match &call.tool {
                AgentTool::Finish { summary } => {
                    let outcome = AgentOutcome::Finished {
                        summary: summary.clone(),
                    };
                    return Ok(report(outcome, turns, calls));
                }
                AgentTool::AskUser { question } => {
                    if let Some(answer) = user.answer(question) {
                        messages.push(tool_message(&call.id, format!("User answered: {answer}")));
                        asked = true;
                        continue;
                    }
                    let outcome = AgentOutcome::AskedUser {
                        question: question.clone(),
                    };
                    return Ok(report(outcome, turns, calls));
                }
                _ => {}
            }
            if let Some(limit) = exhausted(&budget, turns - 1, calls, started) {
                return Ok(report(AgentOutcome::BudgetExhausted(limit), turns, calls));
            }
            if let AgentTool::Click { element_ref } | AgentTool::SetValue { element_ref, .. } =
                &call.tool
                && elements.get(element_ref).is_none()
            {
                messages.push(tool_message(
                    &call.id,
                    format!(
                        "error: ref `{}` was not observed since the window last changed; observe first",
                        escape_text(element_ref)
                    ),
                ));
                continue;
            }
            if let Some(step) = step_for(&call.tool, desktop, &elements)
                && gate_step(&step, user) == StepGate::StoppedByUser
            {
                return Ok(report(AgentOutcome::StoppedByUser, turns, calls));
            }
            calls += 1;
            let result = execute(&call.tool, desktop, &mut elements);
            if matches!(
                call.tool,
                AgentTool::Click { .. } | AgentTool::SetValue { .. } | AgentTool::Press { .. }
            ) {
                // The window may have changed: refs and facts are stale until observed again.
                elements.invalidate();
            }
            let content = match result {
                Ok(text) => text,
                Err(error) => format!("error: {}", frame_untrusted(&error.to_string())),
            };
            messages.push(tool_message(&call.id, content));
        }
    }
}

fn execute(
    tool: &AgentTool,
    desktop: &mut impl AgentDesktop,
    elements: &mut Elements,
) -> Result<String, DeskActError> {
    Ok(match tool {
        AgentTool::Observe { root } => {
            let observation = desktop.observe(root.as_deref())?;
            elements.record(&observation, root.as_deref());
            frame_untrusted(&observation.text)
        }
        AgentTool::Click { element_ref } => {
            desktop.click(element_ref)?;
            "ok".into()
        }
        AgentTool::SetValue { element_ref, text } => {
            desktop.set_value(element_ref, text)?;
            "ok".into()
        }
        AgentTool::Press { keys } => {
            desktop.press(keys)?;
            "ok".into()
        }
        AgentTool::ReadValue { element_ref } => frame_untrusted(&desktop.read_value(element_ref)?),
        AgentTool::Finish { .. } | AgentTool::AskUser { .. } => "ok".into(),
    })
}
