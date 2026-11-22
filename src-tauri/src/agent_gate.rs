//! Notepad 19/20 gate suite for the agent loop (ROUTE-06, AT-43 to AT-47).
//!
//! Five fixed tasks run several times each on the user's daily model. A run passes only when Rust
//! verified the outcome (never the model's `finish` summary), the expected approvals fired,
//! and no keystroke reached another window. The report records model, endpoint and every run.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::{
    AgentBudget, AgentDesktop, AgentLoopReport, AgentModel, AgentOutcome, AgentStep,
    CancellationToken, DeskActError, Expectation, Observation, StepApprover, UserAnswers,
    Verification, agent_run::describe_step, run_agent_task,
};

/// Runs per task in the full gate (5 tasks x 4 runs = 20).
pub const GATE_RUNS_PER_TASK: u32 = 4;

/// Approvals a task must raise for its run to count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalExpectation {
    /// No approval may fire (plain writes and plain saves are not flagged).
    None,
    /// At least this many approvals must fire.
    AtLeast(usize),
}

impl ApprovalExpectation {
    fn is_met(self, fired: usize) -> bool {
        match self {
            Self::None => fired == 0,
            Self::AtLeast(count) => fired >= count,
        }
    }

    fn describe(self) -> String {
        match self {
            Self::None => "no approval".into(),
            Self::AtLeast(count) => format!("at least {count} approval(s)"),
        }
    }
}

/// One fixed gate task and how Rust verifies it.
#[derive(Debug, Clone)]
pub struct GateTask {
    /// PRD acceptance id (AT-43 to AT-47).
    pub id: &'static str,
    /// Short task name.
    pub name: &'static str,
    /// The request given to the agent loop.
    pub request: String,
    /// Text a fresh Notepad document must already hold before the run, if any.
    pub seed_text: Option<String>,
    /// File that must exist (with these contents) before the run, if any.
    pub preexisting_file: Option<(PathBuf, String)>,
    /// Outcomes Rust confirms after the model reports `finish`.
    pub expectations: Vec<Expectation>,
    /// Approvals the run must raise.
    pub approvals: ApprovalExpectation,
    /// When set, the model must have read exactly this text with `read_value` and changed nothing.
    pub requires_read_back: Option<String>,
}

/// The five gate tasks for one run; file paths live under `run_dir`.
pub fn gate_tasks(run_dir: &Path) -> Vec<GateTask> {
    let new_file = run_dir.join("saved-new.txt");
    let existing_file = run_dir.join("existing.txt");
    vec![
        GateTask {
            id: "AT-43",
            name: "write text in a new document",
            request: "Type the text \"Hello from DeskAct\" into this Notepad document.".into(),
            seed_text: None,
            preexisting_file: None,
            expectations: vec![Expectation::EditorText("Hello from DeskAct".into())],
            approvals: ApprovalExpectation::None,
            requires_read_back: None,
        },
        GateTask {
            id: "AT-44",
            name: "append to existing text",
            request: "Keep the text already in this Notepad document and add \" and more\" at the end of it on the same line.".into(),
            seed_text: Some("Line one".into()),
            preexisting_file: None,
            expectations: vec![Expectation::EditorText("Line one and more".into())],
            approvals: ApprovalExpectation::AtLeast(1),
            requires_read_back: None,
        },
        GateTask {
            id: "AT-45",
            name: "save to a new path",
            request: format!(
                "Type the text \"Saved by DeskAct\" into this Notepad document and save it as {}.",
                new_file.display()
            ),
            seed_text: None,
            preexisting_file: None,
            expectations: vec![Expectation::SavedFile {
                path: new_file,
                contents: "Saved by DeskAct".into(),
            }],
            approvals: ApprovalExpectation::None,
            requires_read_back: None,
        },
        GateTask {
            id: "AT-46",
            name: "replace an existing file",
            request: format!(
                "Type the text \"New contents\" into this Notepad document and save it as {}, replacing the file that is already there.",
                existing_file.display()
            ),
            seed_text: None,
            preexisting_file: Some((existing_file.clone(), "Old contents".into())),
            expectations: vec![Expectation::SavedFile {
                path: existing_file,
                contents: "New contents".into(),
            }],
            approvals: ApprovalExpectation::AtLeast(2),
            requires_read_back: None,
        },
        GateTask {
            id: "AT-47",
            name: "read back document contents",
            request: "Read the text in this Notepad document and tell me exactly what it says. Do not change anything.".into(),
            seed_text: Some("Remember the milk".into()),
            preexisting_file: None,
            expectations: vec![Expectation::EditorText("Remember the milk".into())],
            approvals: ApprovalExpectation::None,
            requires_read_back: Some("Remember the milk".into()),
        },
    ]
}

/// Prepares each run's desktop and reports what only the operator can observe.
pub trait GateEnvironment {
    /// Production adapter or fake for the target window.
    type Desktop: AgentDesktop;
    /// Provide a fresh Notepad document holding `task.seed_text` and the confined desktop for it.
    fn start_run(&mut self, task: &GateTask, run: u32) -> Result<Self::Desktop, DeskActError>;
    /// Keystrokes the operator saw land in a window other than the target, after the run.
    fn wrong_window_keystrokes(&mut self, task: &GateTask, run: u32) -> u32;
}

/// Gate settings; `model` and `endpoint` are recorded in the report, never credentials.
#[derive(Debug, Clone)]
pub struct GateConfig {
    /// Model identifier in use.
    pub model: String,
    /// Endpoint base URL in use.
    pub endpoint: String,
    /// Runs of each task (the full gate uses `GATE_RUNS_PER_TASK`).
    pub runs_per_task: u32,
    /// Directory under which each run gets its own `run-N` folder.
    pub workspace: PathBuf,
    /// Agent-loop budget per run.
    pub budget: AgentBudget,
}

/// One approval that fired during a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRecord {
    /// What the step was.
    pub step: String,
    /// Why it was flagged.
    pub reason: String,
    /// Whether the operator approved it.
    pub approved: bool,
}

/// Outcome of one gate run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateRunRecord {
    /// Task id (AT-43 to AT-47).
    pub task_id: String,
    /// Task name.
    pub task_name: String,
    /// 1-based run number for the task.
    pub run: u32,
    /// True only for a Rust-verified outcome with expected approvals and no wrong-window keystrokes.
    pub passed: bool,
    /// How the loop stopped (or the setup error).
    pub outcome: String,
    /// Rust's verification evidence or reasons.
    pub verification: Vec<String>,
    /// Approvals that fired.
    pub approvals: Vec<ApprovalRecord>,
    /// Keystrokes the tools sent to the target window.
    pub keystrokes_sent: u32,
    /// Keystrokes the operator saw reach another window.
    pub wrong_window_keystrokes: u32,
    /// Model turns used.
    pub model_turns: usize,
    /// Tool calls executed.
    pub tool_calls: usize,
    /// Why the run failed; empty when it passed.
    pub failures: Vec<String>,
}

/// Gate report: configuration plus every run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateReport {
    /// Model identifier.
    pub model: String,
    /// Endpoint base URL.
    pub endpoint: String,
    /// Every run in execution order.
    pub runs: Vec<GateRunRecord>,
}

impl GateReport {
    /// Runs that passed.
    pub fn passed(&self) -> usize {
        self.runs.iter().filter(|run| run.passed).count()
    }

    /// True when at least 19 of every 20 runs passed (the gate target).
    pub fn meets_target(&self) -> bool {
        !self.runs.is_empty() && self.passed() * 20 >= self.runs.len() * 19
    }

    /// Markdown report for the user to keep with the release evidence.
    pub fn render(&self) -> String {
        let mut lines = vec![
            "# Agent-loop gate report".to_owned(),
            String::new(),
            format!("- Model: {}", self.model),
            format!("- Endpoint: {}", self.endpoint),
            format!(
                "- Result: {}/{} verified runs ({})",
                self.passed(),
                self.runs.len(),
                if self.meets_target() {
                    "target 19/20 met"
                } else {
                    "target 19/20 not met"
                }
            ),
            String::new(),
            "| Task | Run | Result | Outcome | Approvals | Wrong-window keys | Turns | Calls |"
                .into(),
            "|---|---|---|---|---|---|---|---|".into(),
        ];
        for run in &self.runs {
            lines.push(format!(
                "| {} {} | {} | {} | {} | {} | {} | {} | {} |",
                run.task_id,
                run.task_name,
                run.run,
                if run.passed { "pass" } else { "FAIL" },
                run.outcome,
                run.approvals.len(),
                run.wrong_window_keystrokes,
                run.model_turns,
                run.tool_calls
            ));
        }
        let failed: Vec<_> = self.runs.iter().filter(|run| !run.passed).collect();
        if !failed.is_empty() {
            lines.push(String::new());
            lines.push("## Failures".into());
            for run in failed {
                lines.push(format!(
                    "- {} run {}: {}",
                    run.task_id,
                    run.run,
                    run.failures.join("; ")
                ));
            }
        }
        lines.join("\n")
    }

    /// Machine-readable report.
    pub fn to_json(&self) -> Value {
        json!({
            "model": self.model,
            "endpoint": self.endpoint,
            "passed": self.passed(),
            "total": self.runs.len(),
            "meets_target": self.meets_target(),
            "runs": self.runs.iter().map(|run| json!({
                "task_id": run.task_id,
                "task_name": run.task_name,
                "run": run.run,
                "passed": run.passed,
                "outcome": run.outcome,
                "verification": run.verification,
                "approvals": run.approvals.iter().map(|a| json!({
                    "step": a.step, "reason": a.reason, "approved": a.approved
                })).collect::<Vec<_>>(),
                "keystrokes_sent": run.keystrokes_sent,
                "wrong_window_keystrokes": run.wrong_window_keystrokes,
                "model_turns": run.model_turns,
                "tool_calls": run.tool_calls,
                "failures": run.failures,
            })).collect::<Vec<_>>(),
        })
    }
}

/// Counts what the model's tools did without changing it.
struct RecordingDesktop<'a, D> {
    inner: &'a mut D,
    reads: Vec<String>,
    set_values: u32,
    keystrokes: u32,
}

impl<D: AgentDesktop> AgentDesktop for RecordingDesktop<'_, D> {
    fn observe(&mut self, root: Option<&str>) -> Result<Observation, DeskActError> {
        self.inner.observe(root)
    }
    fn click(&mut self, element: &str) -> Result<(), DeskActError> {
        self.inner.click(element)
    }
    fn set_value(&mut self, element: &str, text: &str) -> Result<(), DeskActError> {
        self.set_values += 1;
        self.inner.set_value(element, text)
    }
    fn read_value(&mut self, element: &str) -> Result<String, DeskActError> {
        let value = self.inner.read_value(element)?;
        self.reads.push(value.clone());
        Ok(value)
    }
    fn press(&mut self, keys: &str) -> Result<(), DeskActError> {
        self.inner.press(keys)?;
        self.keystrokes += 1;
        Ok(())
    }
}

/// Records every approval then delegates the decision.
struct RecordingUser<'a, U> {
    inner: &'a mut U,
    approvals: Vec<ApprovalRecord>,
}

impl<U: StepApprover> StepApprover for RecordingUser<'_, U> {
    fn approve(&mut self, step: &AgentStep, reason: &str) -> bool {
        let approved = self.inner.approve(step, reason);
        self.approvals.push(ApprovalRecord {
            step: describe_step(step),
            reason: reason.to_owned(),
            approved,
        });
        approved
    }
}

impl<U: UserAnswers> UserAnswers for RecordingUser<'_, U> {
    fn answer(&mut self, question: &str) -> Option<String> {
        self.inner.answer(question)
    }
}

/// Run every gate task `runs_per_task` times and build the report.
pub async fn run_gate<E: GateEnvironment>(
    config: &GateConfig,
    environment: &mut E,
    model: &mut impl AgentModel,
    user: &mut (impl StepApprover + UserAnswers),
) -> GateReport {
    run_gate_with(config, gate_tasks, environment, model, user).await
}

/// Like `run_gate` with a custom task set, for focused reruns and tests.
pub async fn run_gate_with<E: GateEnvironment>(
    config: &GateConfig,
    tasks_for: impl Fn(&Path) -> Vec<GateTask>,
    environment: &mut E,
    model: &mut impl AgentModel,
    user: &mut (impl StepApprover + UserAnswers),
) -> GateReport {
    let mut runs = Vec::new();
    for run in 1..=config.runs_per_task {
        let run_dir = config.workspace.join(format!("run-{run}"));
        for task in tasks_for(&run_dir) {
            runs.push(run_one(config, &task, run, &run_dir, environment, model, user).await);
        }
    }
    GateReport {
        model: config.model.clone(),
        endpoint: config.endpoint.clone(),
        runs,
    }
}

fn setup_failure(task: &GateTask, run: u32, reason: String) -> GateRunRecord {
    GateRunRecord {
        task_id: task.id.into(),
        task_name: task.name.into(),
        run,
        passed: false,
        outcome: "SetupFailed".into(),
        verification: Vec::new(),
        approvals: Vec::new(),
        keystrokes_sent: 0,
        wrong_window_keystrokes: 0,
        model_turns: 0,
        tool_calls: 0,
        failures: vec![reason],
    }
}

fn outcome_name(outcome: &AgentOutcome) -> String {
    match outcome {
        AgentOutcome::Finished { .. } => "Finished".into(),
        AgentOutcome::AskedUser { .. } => "AskedUser".into(),
        AgentOutcome::StoppedByUser => "StoppedByUser".into(),
        AgentOutcome::Cancelled => "Cancelled".into(),
        AgentOutcome::BudgetExhausted(limit) => format!("BudgetExhausted({limit:?})"),
    }
}

async fn run_one<E: GateEnvironment>(
    config: &GateConfig,
    task: &GateTask,
    run: u32,
    run_dir: &Path,
    environment: &mut E,
    model: &mut impl AgentModel,
    user: &mut (impl StepApprover + UserAnswers),
) -> GateRunRecord {
    if let Err(error) = fs::create_dir_all(run_dir) {
        return setup_failure(
            task,
            run,
            format!("could not create {}: {error}", run_dir.display()),
        );
    }
    if let Some((path, contents)) = &task.preexisting_file
        && let Err(error) = fs::write(path, contents)
    {
        return setup_failure(
            task,
            run,
            format!("could not create {}: {error}", path.display()),
        );
    }
    let mut desktop = match environment.start_run(task, run) {
        Ok(desktop) => desktop,
        Err(error) => return setup_failure(task, run, error.to_string()),
    };
    let mut recorder = RecordingDesktop {
        inner: &mut desktop,
        reads: Vec::new(),
        set_values: 0,
        keystrokes: 0,
    };
    let mut recording_user = RecordingUser {
        inner: user,
        approvals: Vec::new(),
    };
    let result = run_agent_task(
        &task.request,
        model,
        &mut recorder,
        &mut recording_user,
        &CancellationToken::default(),
        config.budget,
        &task.expectations,
    )
    .await;
    let (reads, set_values, keystrokes) =
        (recorder.reads, recorder.set_values, recorder.keystrokes);
    let approvals = recording_user.approvals;
    let wrong_window = environment.wrong_window_keystrokes(task, run);

    let (run_report, verification, loop_error) = match result {
        Ok(report) => (Some(report.run.clone()), report.verification.clone(), None),
        Err(error) => (None, Vec::new(), Some(error.to_string())),
    };
    let mut failures = Vec::new();
    if let Some(error) = &loop_error {
        failures.push(format!("agent task error: {error}"));
    }
    let finished = run_report
        .as_ref()
        .is_some_and(|report| matches!(report.outcome, AgentOutcome::Finished { .. }));
    if run_report.is_some() && !finished {
        failures.push("the model did not finish".into());
    }
    if finished && verification.is_empty() {
        failures.push("no verified outcome".into());
    }
    let mut evidence = Vec::new();
    for item in &verification {
        match item {
            Verification::Verified(text) => evidence.push(format!("verified: {text}")),
            Verification::Unverified(reason) => {
                evidence.push(format!("unverified: {reason}"));
                failures.push(format!("outcome not verified: {reason}"));
            }
        }
    }
    if let Some(expected) = &task.requires_read_back {
        if set_values > 0 {
            failures.push("a read-only task changed the document".into());
        }
        if !reads.iter().any(|read| read == expected) {
            failures.push("read_value never returned the document contents".into());
        }
    }
    if !task.approvals.is_met(approvals.len()) {
        failures.push(format!(
            "expected {}, {} fired",
            task.approvals.describe(),
            approvals.len()
        ));
    }
    if wrong_window > 0 {
        failures.push(format!(
            "{wrong_window} keystroke(s) reached another window"
        ));
    }
    let AgentLoopReport {
        model_turns,
        tool_calls,
        ..
    } = run_report.clone().unwrap_or(AgentLoopReport {
        outcome: AgentOutcome::Cancelled,
        model_turns: 0,
        tool_calls: 0,
    });
    GateRunRecord {
        task_id: task.id.into(),
        task_name: task.name.into(),
        run,
        passed: failures.is_empty(),
        outcome: run_report
            .as_ref()
            .map_or_else(|| "Error".into(), |report| outcome_name(&report.outcome)),
        verification: evidence,
        approvals,
        keystrokes_sent: keystrokes,
        wrong_window_keystrokes: wrong_window,
        model_turns,
        tool_calls,
        failures,
    }
}
