//! Gate runner tests: task definitions, verified-only counting, approvals and wrong-window records.

use std::{fs, path::PathBuf};

use deskact_core::{
    AgentBudget, AgentDesktop, AgentModel, AgentStep, AgentTool, AgentTurn, ApprovalExpectation,
    DeskActError, GateConfig, GateEnvironment, GateTask, Observation, StepApprover,
    ToolCallProposal, UserAnswers, gate_tasks, run_gate,
};
use serde_json::{Value, json};

const SNAPSHOT: &str = r##"{"window":{"id":"w"},"tree":{"role":"window","name":"Notepad","children":[
  {"role":"textfield","name":"Text editor","ref_id":"@e1","value":"","states":["focused"]},
  {"role":"dialog","name":"Save As","children":[
    {"role":"textfield","name":"File name","ref_id":"@e5"},
    {"role":"button","name":"Save","ref_id":"@e6"}]},
  {"role":"dialog","name":"Confirm overwrite","children":[
    {"role":"button","name":"Yes","ref_id":"@e7"}]}]}}"##;

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime")
        .block_on(future)
}

fn workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("deskact-gate-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

/// Notepad stand-in: one editor plus a Save As flow that writes the editor text to disk.
#[derive(Default)]
struct FakeNotepad {
    editor: String,
    file_name: String,
}

impl AgentDesktop for FakeNotepad {
    fn observe(&mut self, _root: Option<&str>) -> Result<Observation, DeskActError> {
        Ok(Observation {
            text: SNAPSHOT.into(),
            truncated: false,
        })
    }
    fn click(&mut self, element: &str) -> Result<(), DeskActError> {
        let path = PathBuf::from(&self.file_name);
        match element {
            "@e6" if !path.exists() => fs::write(path, &self.editor).expect("write"),
            "@e7" => fs::write(path, &self.editor).expect("write"),
            _ => {}
        }
        Ok(())
    }
    fn set_value(&mut self, element: &str, text: &str) -> Result<(), DeskActError> {
        if element == "@e5" {
            self.file_name = text.into();
        } else {
            self.editor = text.into();
        }
        Ok(())
    }
    fn read_value(&mut self, element: &str) -> Result<String, DeskActError> {
        Ok(if element == "@e5" {
            self.file_name.clone()
        } else {
            self.editor.clone()
        })
    }
    fn press(&mut self, _keys: &str) -> Result<(), DeskActError> {
        Ok(())
    }
}

#[derive(Default)]
struct FakeEnvironment {
    wrong_window: Vec<(String, u32)>,
    started: Vec<String>,
}

impl GateEnvironment for FakeEnvironment {
    type Desktop = FakeNotepad;
    fn start_run(&mut self, task: &GateTask, run: u32) -> Result<FakeNotepad, DeskActError> {
        self.started.push(format!("{}#{run}", task.id));
        Ok(FakeNotepad {
            editor: task.seed_text.clone().unwrap_or_default(),
            file_name: String::new(),
        })
    }
    fn wrong_window_keystrokes(&mut self, task: &GateTask, run: u32) -> u32 {
        self.wrong_window
            .iter()
            .filter(|(id, r)| id == task.id && *r == run)
            .count() as u32
    }
}

/// Model that does each gate task correctly, found by matching the request in the conversation.
struct CompetentModel {
    tasks: Vec<GateTask>,
    claim_without_working_for: Option<&'static str>,
}

impl CompetentModel {
    fn new(workspace: &std::path::Path) -> Self {
        let tasks = (1..=4)
            .flat_map(|run| gate_tasks(&workspace.join(format!("run-{run}"))))
            .collect();
        Self {
            tasks,
            claim_without_working_for: None,
        }
    }
}

fn editor_text(task: &GateTask) -> String {
    task.expectations
        .iter()
        .map(|e| match e {
            deskact_core::Expectation::EditorText(text) => text.clone(),
            deskact_core::Expectation::SavedFile { contents, .. } => contents.clone(),
        })
        .next()
        .expect("text")
}

fn saved_path(task: &GateTask) -> String {
    task.expectations
        .iter()
        .find_map(|e| match e {
            deskact_core::Expectation::SavedFile { path, .. } => Some(path.display().to_string()),
            _ => None,
        })
        .expect("path")
}

impl AgentModel for CompetentModel {
    async fn next_turn(&mut self, messages: &[Value]) -> Result<AgentTurn, DeskActError> {
        let task = self
            .tasks
            .iter()
            .find(|task| {
                messages.iter().any(|m| {
                    m["role"] == "user"
                        && m["content"]
                            .as_str()
                            .is_some_and(|c| c.contains(&task.request))
                })
            })
            .expect("known request");
        let step = messages
            .iter()
            .filter(|m| {
                m["tool_call_id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with('c'))
            })
            .count();
        let set = |element: &str, text: String| AgentTool::SetValue {
            element_ref: element.into(),
            text,
        };
        let click = |element: &str| AgentTool::Click {
            element_ref: element.into(),
        };
        let mut script = match task.id {
            "AT-43" => vec![set("@e1", editor_text(task))],
            "AT-44" => vec![set("@e1", editor_text(task))],
            "AT-45" | "AT-46" => vec![
                set("@e1", editor_text(task)),
                AgentTool::Press {
                    keys: "ctrl+shift+s".into(),
                },
                set("@e5", saved_path(task)),
                click("@e6"),
            ],
            _ => vec![AgentTool::ReadValue {
                element_ref: "@e1".into(),
            }],
        };
        if task.id == "AT-46" {
            script.push(click("@e7"));
        }
        script.push(AgentTool::Finish {
            summary: "done".into(),
        });
        if Some(task.id) == self.claim_without_working_for {
            script = vec![AgentTool::Finish {
                summary: "claimed done".into(),
            }];
        }
        let tool = script[step.min(script.len() - 1)].clone();
        Ok(AgentTurn {
            tool_calls: vec![ToolCallProposal {
                id: format!("c{step}"),
                tool,
            }],
            content: None,
            assistant_message: json!({"role": "assistant", "content": null}),
        })
    }
}

struct Operator {
    approvals: bool,
}

impl StepApprover for Operator {
    fn approve(&mut self, _step: &AgentStep, _reason: &str) -> bool {
        self.approvals
    }
}

impl UserAnswers for Operator {
    fn answer(&mut self, _question: &str) -> Option<String> {
        None
    }
}

fn config(dir: PathBuf, runs: u32) -> GateConfig {
    GateConfig {
        model: "daily-model".into(),
        endpoint: "https://llm.example/v1".into(),
        runs_per_task: runs,
        workspace: dir,
        budget: AgentBudget::default(),
    }
}

#[test]
fn defines_the_five_gate_tasks_with_workspace_scoped_paths() {
    let dir = workspace("defs");
    let tasks = gate_tasks(&dir.join("run-1"));
    let ids: Vec<_> = tasks.iter().map(|task| task.id).collect();
    assert_eq!(ids, ["AT-43", "AT-44", "AT-45", "AT-46", "AT-47"]);
    assert_eq!(tasks[0].approvals, ApprovalExpectation::None);
    assert_eq!(tasks[1].approvals, ApprovalExpectation::AtLeast(1));
    assert_eq!(tasks[3].approvals, ApprovalExpectation::AtLeast(2));
    assert_eq!(tasks[1].seed_text.as_deref(), Some("Line one"));
    assert_eq!(editor_text(&tasks[1]), "Line one and more");
    let saved = saved_path(&tasks[2]);
    assert!(saved.starts_with(&dir.display().to_string()), "{saved}");
    assert!(tasks[2].request.contains(&saved));
    assert!(tasks[3].preexisting_file.is_some());
}

#[test]
fn competent_model_passes_all_twenty_runs_and_meets_the_target() {
    let dir = workspace("all");
    let mut model = CompetentModel::new(&dir);
    let report = block_on(run_gate(
        &config(dir, 4),
        &mut FakeEnvironment::default(),
        &mut model,
        &mut Operator { approvals: true },
    ));

    assert_eq!(report.runs.len(), 20);
    let failures: Vec<_> = report.runs.iter().filter(|r| !r.passed).collect();
    assert!(failures.is_empty(), "{failures:#?}");
    assert_eq!(report.passed(), 20);
    assert!(report.meets_target());
    let at46 = report
        .runs
        .iter()
        .find(|r| r.task_id == "AT-46")
        .expect("run");
    assert_eq!(at46.approvals.len(), 2, "{:?}", at46.approvals);
    let at43 = report
        .runs
        .iter()
        .find(|r| r.task_id == "AT-43")
        .expect("run");
    assert!(at43.approvals.is_empty());
}

#[test]
fn model_claiming_success_without_the_outcome_does_not_pass() {
    let dir = workspace("claim");
    let mut model = CompetentModel::new(&dir);
    model.claim_without_working_for = Some("AT-43");
    let report = block_on(run_gate(
        &config(dir, 4),
        &mut FakeEnvironment::default(),
        &mut model,
        &mut Operator { approvals: true },
    ));

    let at43: Vec<_> = report
        .runs
        .iter()
        .filter(|r| r.task_id == "AT-43")
        .collect();
    assert!(at43.iter().all(|r| !r.passed));
    assert!(at43[0].outcome.contains("Finished"), "{}", at43[0].outcome);
    assert_eq!(report.passed(), 16);
    assert!(!report.meets_target());
}

#[test]
fn a_wrong_window_keystroke_fails_the_run_and_is_recorded() {
    let dir = workspace("wrong");
    let mut model = CompetentModel::new(&dir);
    let mut env = FakeEnvironment::default();
    env.wrong_window.push(("AT-47".into(), 2));
    let report = block_on(run_gate(
        &config(dir, 4),
        &mut env,
        &mut model,
        &mut Operator { approvals: true },
    ));

    let run = report
        .runs
        .iter()
        .find(|r| r.task_id == "AT-47" && r.run == 2)
        .expect("run");
    assert_eq!(run.wrong_window_keystrokes, 1);
    assert!(!run.passed);
    assert_eq!(report.passed(), 19);
    assert!(report.meets_target());
}

#[test]
fn denied_approval_fails_the_run_as_stopped_by_user() {
    let dir = workspace("deny");
    let mut model = CompetentModel::new(&dir);
    let report = block_on(run_gate(
        &config(dir, 1),
        &mut FakeEnvironment::default(),
        &mut model,
        &mut Operator { approvals: false },
    ));

    let at44 = report
        .runs
        .iter()
        .find(|r| r.task_id == "AT-44")
        .expect("run");
    assert!(!at44.passed);
    assert_eq!(at44.outcome, "StoppedByUser");
    assert!(!at44.approvals[0].approved);
}

#[test]
fn report_records_model_endpoint_and_each_run() {
    let dir = workspace("render");
    let mut model = CompetentModel::new(&dir);
    let report = block_on(run_gate(
        &config(dir, 1),
        &mut FakeEnvironment::default(),
        &mut model,
        &mut Operator { approvals: true },
    ));

    let text = report.render();
    assert!(text.contains("daily-model"), "{text}");
    assert!(text.contains("https://llm.example/v1"), "{text}");
    assert!(text.contains("5/5"), "{text}");
    for id in ["AT-43", "AT-44", "AT-45", "AT-46", "AT-47"] {
        assert!(text.contains(id), "{text}");
    }
    let json = report.to_json();
    assert_eq!(json["model"], "daily-model");
    assert_eq!(json["runs"].as_array().expect("runs").len(), 5);
    assert_eq!(json["runs"][0]["wrong_window_keystrokes"], 0);
}
