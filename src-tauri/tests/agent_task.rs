//! Agent-task tests: ask_user resume, verified vs model-reported outcomes, console prompts, routing.

use std::{collections::VecDeque, fs, io::Cursor};

use deskact_core::{
    AgentBudget, AgentDesktop, AgentLoopReport, AgentModel, AgentOutcome, AgentStep, AgentTool,
    AgentTurn, AppCatalogEntry, AppCatalogSource, BudgetLimit, CancellationToken, ConsolePrompts,
    DeskActError, Expectation, Observation, ReportingDesktop, StepApprover, TaskRoute,
    ToolCallProposal, UserAnswers, Verification, route_request, run_agent_loop,
    run_agent_loop_with_answers, run_agent_task,
};
use serde_json::{Value, json};

const SNAPSHOT: &str = r##"{"window":{"id":"w"},"tree":{"role":"window","name":"Untitled - Notepad","children":[
  {"role":"textfield","name":"Text editor","ref_id":"@e1","value":"","states":["focused"]},
  {"role":"button","name":"Bold","ref_id":"@e3"}]}}"##;

#[derive(Default)]
struct FakeDesktop {
    value: String,
    observes: usize,
    snapshot: Option<String>,
}

impl AgentDesktop for FakeDesktop {
    fn observe(&mut self, _root: Option<&str>) -> Result<Observation, DeskActError> {
        self.observes += 1;
        Ok(Observation {
            text: self.snapshot.clone().unwrap_or_else(|| SNAPSHOT.into()),
            truncated: false,
        })
    }
    fn click(&mut self, _element: &str) -> Result<(), DeskActError> {
        Ok(())
    }
    fn set_value(&mut self, _element: &str, text: &str) -> Result<(), DeskActError> {
        self.value = text.into();
        Ok(())
    }
    fn read_value(&mut self, _element: &str) -> Result<String, DeskActError> {
        Ok(self.value.clone())
    }
    fn press(&mut self, _keys: &str) -> Result<(), DeskActError> {
        Ok(())
    }
}

struct ScriptedModel {
    turns: VecDeque<Vec<AgentTool>>,
    seen: Vec<Vec<Value>>,
}

impl ScriptedModel {
    fn new(turns: Vec<Vec<AgentTool>>) -> Self {
        Self {
            turns: turns.into(),
            seen: Vec::new(),
        }
    }
}

impl AgentModel for ScriptedModel {
    async fn next_turn(&mut self, messages: &[Value]) -> Result<AgentTurn, DeskActError> {
        self.seen.push(messages.to_vec());
        let tools = if self.turns.len() > 1 {
            self.turns.pop_front().expect("turn")
        } else {
            self.turns.front().cloned().expect("script ended")
        };
        let n = self.seen.len();
        Ok(AgentTurn {
            tool_calls: tools
                .into_iter()
                .enumerate()
                .map(|(i, tool)| ToolCallProposal {
                    id: format!("call-{n}-{i}"),
                    tool,
                })
                .collect(),
            content: None,
            assistant_message: json!({"role": "assistant", "content": null}),
        })
    }
}

/// Approver and answerer in one, as the loop takes a single user port.
struct User {
    approve: bool,
    answers: Vec<String>,
}

impl StepApprover for User {
    fn approve(&mut self, _step: &AgentStep, _reason: &str) -> bool {
        self.approve
    }
}

impl UserAnswers for User {
    fn answer(&mut self, _question: &str) -> Option<String> {
        if self.answers.is_empty() {
            None
        } else {
            Some(self.answers.remove(0))
        }
    }
}

fn user(approve: bool, answers: Vec<String>) -> User {
    User { approve, answers }
}

#[derive(Default)]
struct Approver;

impl StepApprover for Approver {
    fn approve(&mut self, _step: &AgentStep, _reason: &str) -> bool {
        false
    }
}

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime")
        .block_on(future)
}

fn finish() -> AgentTool {
    AgentTool::Finish {
        summary: "typed it".into(),
    }
}

fn ask(question: &str) -> AgentTool {
    AgentTool::AskUser {
        question: question.into(),
    }
}

fn set_value(text: &str) -> AgentTool {
    AgentTool::SetValue {
        element_ref: "@e1".into(),
        text: text.into(),
    }
}

fn run_with_answers(
    model: &mut ScriptedModel,
    desktop: &mut FakeDesktop,
    user: &mut User,
    budget: AgentBudget,
) -> AgentLoopReport {
    block_on(run_agent_loop_with_answers(
        "write hello",
        model,
        desktop,
        user,
        &CancellationToken::default(),
        budget,
    ))
    .expect("loop")
}

#[test]
fn answered_ask_user_continues_the_same_loop() {
    let mut model = ScriptedModel::new(vec![vec![ask("Which text?")], vec![finish()]]);
    let report = run_with_answers(
        &mut model,
        &mut FakeDesktop::default(),
        &mut user(true, vec!["hello".into()]),
        AgentBudget::default(),
    );

    assert_eq!(
        report.outcome,
        AgentOutcome::Finished {
            summary: "typed it".into()
        }
    );
    assert_eq!(report.model_turns, 2);
    let second_turn = model.seen[1].last().expect("message").to_string();
    assert!(second_turn.contains("hello"), "{second_turn}");
    assert!(second_turn.contains("call-1-0"), "{second_turn}");
}

#[test]
fn answers_spend_the_same_budget() {
    let mut model = ScriptedModel::new(vec![vec![ask("Again?")]]);
    let budget = AgentBudget {
        max_model_turns: 3,
        ..AgentBudget::default()
    };
    let report = run_with_answers(
        &mut model,
        &mut FakeDesktop::default(),
        &mut user(true, vec!["a".into(), "b".into(), "c".into(), "d".into()]),
        budget,
    );

    assert_eq!(
        report.outcome,
        AgentOutcome::BudgetExhausted(BudgetLimit::ModelTurns)
    );
    assert_eq!(report.model_turns, 3);
}

#[test]
fn unanswered_ask_user_still_ends_the_run() {
    let mut model = ScriptedModel::new(vec![vec![ask("Which text?")]]);
    let report = block_on(run_agent_loop(
        "write hello",
        &mut model,
        &mut FakeDesktop::default(),
        &mut Approver,
        &CancellationToken::default(),
        AgentBudget::default(),
    ))
    .expect("loop");

    assert_eq!(
        report.outcome,
        AgentOutcome::AskedUser {
            question: "Which text?".into()
        }
    );
}

#[test]
fn calls_after_an_answered_ask_user_are_skipped_but_answered() {
    let mut desktop = FakeDesktop::default();
    let mut model = ScriptedModel::new(vec![
        vec![
            ask("Which text?"),
            AgentTool::Observe { root: None },
            set_value("x"),
        ],
        vec![finish()],
    ]);
    run_with_answers(
        &mut model,
        &mut desktop,
        &mut user(true, vec!["hello".into()]),
        AgentBudget::default(),
    );

    assert_eq!(desktop.observes, 1, "only the initial observation ran");
    assert_eq!(desktop.value, "");
    let tool_results = model.seen[1]
        .iter()
        .filter(|message| message["role"] == "tool")
        .count();
    assert_eq!(tool_results, 3);
}

fn run_task(
    model: &mut ScriptedModel,
    desktop: &mut FakeDesktop,
    approve: bool,
    expectations: Vec<Expectation>,
) -> deskact_core::AgentTaskReport {
    block_on(run_agent_task(
        "write hello",
        model,
        desktop,
        &mut user(approve, Vec::new()),
        &CancellationToken::default(),
        AgentBudget::default(),
        &expectations,
    ))
    .expect("task")
}

#[test]
fn editor_text_matching_the_expectation_is_a_verified_outcome() {
    let mut model = ScriptedModel::new(vec![vec![set_value("hello")], vec![finish()]]);
    let report = run_task(
        &mut model,
        &mut FakeDesktop::default(),
        true,
        vec![Expectation::EditorText("hello".into())],
    );

    assert!(report.is_verified());
    assert!(matches!(
        report.verification[..],
        [Verification::Verified(_)]
    ));
    let text = report.render();
    assert!(text.contains("Verified outcome"), "{text}");
}

#[test]
fn editor_text_mismatch_is_unverified_even_when_the_model_says_done() {
    let mut model = ScriptedModel::new(vec![vec![set_value("hellp")], vec![finish()]]);
    let report = run_task(
        &mut model,
        &mut FakeDesktop::default(),
        true,
        vec![Expectation::EditorText("hello".into())],
    );

    assert!(!report.is_verified());
    let text = report.render();
    assert!(text.contains("Unverified"), "{text}");
    assert!(!text.contains("Verified outcome"), "{text}");
}

#[test]
fn truncated_or_ambiguous_observation_is_unverified() {
    let mut desktop = FakeDesktop {
        snapshot: Some(r##"{"tree":{"role":"window","name":"w","children":[
          {"role":"textfield","name":"a","ref_id":"@e1"},{"role":"textfield","name":"b","ref_id":"@e2"}]}}"##.into()),
        ..FakeDesktop::default()
    };
    let mut model = ScriptedModel::new(vec![vec![finish()]]);
    let report = run_task(
        &mut model,
        &mut desktop,
        true,
        vec![Expectation::EditorText(String::new())],
    );

    assert!(!report.is_verified());
}

#[test]
fn finish_without_an_expectation_is_labelled_model_reported() {
    let mut model = ScriptedModel::new(vec![vec![finish()]]);
    let report = run_task(&mut model, &mut FakeDesktop::default(), true, Vec::new());

    assert!(!report.is_verified());
    let text = report.render();
    assert!(
        text.contains("Unverified (model-reported): typed it"),
        "{text}"
    );
}

#[test]
fn saved_file_is_verified_by_reading_it_from_disk() {
    let dir = std::env::temp_dir().join(format!("deskact-agent-task-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("out.txt");
    fs::write(&path, "hello").expect("write");
    let expectation = |contents: &str| Expectation::SavedFile {
        path: path.clone(),
        contents: contents.into(),
    };

    let mut model = ScriptedModel::new(vec![vec![finish()]]);
    let good = run_task(
        &mut model,
        &mut FakeDesktop::default(),
        true,
        vec![expectation("hello")],
    );
    let mut model = ScriptedModel::new(vec![vec![finish()]]);
    let bad = run_task(
        &mut model,
        &mut FakeDesktop::default(),
        true,
        vec![expectation("goodbye")],
    );
    fs::remove_dir_all(&dir).expect("cleanup");

    assert!(good.is_verified());
    assert!(!bad.is_verified());
}

#[test]
fn denied_step_reports_stopped_by_user_without_verifying() {
    let mut desktop = FakeDesktop {
        value: "existing".into(),
        ..FakeDesktop::default()
    };
    let mut model = ScriptedModel::new(vec![vec![set_value("hello")], vec![finish()]]);
    let report = run_task(
        &mut model,
        &mut desktop,
        false,
        vec![Expectation::EditorText("hello".into())],
    );

    assert_eq!(report.run.outcome, AgentOutcome::StoppedByUser);
    assert!(report.verification.is_empty());
    assert!(!report.is_verified());
    assert!(report.render().contains("Stopped by user"));
    assert_eq!(desktop.value, "existing");
}

#[test]
fn budget_exhaustion_is_reported_partial_and_unverified() {
    let mut model = ScriptedModel::new(vec![vec![AgentTool::Observe { root: None }]]);
    let report = block_on(run_agent_task(
        "write hello",
        &mut model,
        &mut FakeDesktop::default(),
        &mut user(true, Vec::new()),
        &CancellationToken::default(),
        AgentBudget {
            max_model_turns: 2,
            ..AgentBudget::default()
        },
        &[Expectation::EditorText("hello".into())],
    ))
    .expect("task");

    assert!(!report.is_verified());
    assert!(report.render().contains("Partial and unverified"));
}

#[test]
fn console_prompts_approve_only_on_explicit_yes() {
    let step = AgentStep::Click {
        element_name: "Delete file".into(),
        in_confirmation_dialog: false,
    };
    let answers = |input: &str| {
        let mut output = Vec::new();
        let approved = ConsolePrompts::new(Cursor::new(input.to_owned()), &mut output)
            .approve(&step, "targets an element named \"Delete file\"");
        (approved, String::from_utf8(output).expect("utf8"))
    };

    let (yes, shown) = answers("y\n");
    assert!(yes);
    assert!(shown.contains("Delete file"), "{shown}");
    assert!(answers("YES\n").0);
    assert!(!answers("n\n").0);
    assert!(!answers("\n").0);
    assert!(!answers("").0, "end of input denies");
}

#[test]
fn console_prompts_collect_an_answer_or_give_up() {
    let mut output = Vec::new();
    let mut prompts = ConsolePrompts::new(Cursor::new("report.txt\n\n"), &mut output);
    assert_eq!(prompts.answer("Which file?").as_deref(), Some("report.txt"));
    assert_eq!(prompts.answer("Which file?"), None, "empty answer stops");
    assert_eq!(prompts.answer("Which file?"), None, "end of input stops");
    assert!(
        String::from_utf8(output)
            .expect("utf8")
            .contains("Which file?")
    );
}

#[test]
fn reporting_desktop_prints_progress_for_executed_actions() {
    let mut output = Vec::new();
    {
        let mut desktop = ReportingDesktop::new(FakeDesktop::default(), &mut output);
        desktop.observe(None).expect("observe");
        desktop.set_value("@e1", "secret text").expect("set");
        desktop.press("ctrl+s").expect("press");
    }
    let shown = String::from_utf8(output).expect("utf8");
    assert!(shown.contains("observe"), "{shown}");
    assert!(shown.contains("set_value @e1"), "{shown}");
    assert!(shown.contains("press ctrl+s"), "{shown}");
}

fn entry(name: &str) -> AppCatalogEntry {
    AppCatalogEntry {
        id: format!("id-{name}"),
        name: name.into(),
        target: format!("C:\\\\Windows\\\\{name}.exe"),
        arguments: String::new(),
        working_directory: String::new(),
        source: AppCatalogSource::UserConfigured,
        aliases: Vec::new(),
    }
}

#[test]
fn open_app_requests_keep_the_fast_path_and_everything_else_uses_the_loop() {
    let catalog = [entry("Notepad")];
    assert_eq!(route_request("open notepad", &catalog), TaskRoute::OpenApp);
    assert_eq!(
        route_request("open notepad and type hello", &catalog),
        TaskRoute::AgentLoop
    );
    assert_eq!(
        route_request("write hello in the document", &catalog),
        TaskRoute::AgentLoop
    );
}
