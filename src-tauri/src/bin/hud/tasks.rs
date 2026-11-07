use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};

use deskact_core::{
    DeskActError, NativeTargetResolution, ResolvedNativeTarget, TaskExecutionReport,
    VerifiedNativeCli, execute_prepared_task, prepare_task_request,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

pub const TASK_UPDATED_EVENT: &str = "deskact://task-updated";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPhase {
    Planning,
    Running,
    Completed,
    Partial,
    Failed,
}

impl TaskPhase {
    fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Partial | Self::Failed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskSnapshot {
    pub task_id: u64,
    pub revision: u64,
    pub request: String,
    pub phase: TaskPhase,
    pub message: String,
    pub report: Option<TaskExecutionReport>,
    pub test_fixture: bool,
}

struct ActiveTask {
    snapshot: TaskSnapshot,
}

#[derive(Default)]
struct HubState {
    active: Option<ActiveTask>,
    last_result: Option<TaskSnapshot>,
}

#[derive(Default)]
pub struct TaskHub {
    next_task_id: AtomicU64,
    next_revision: AtomicU64,
    state: Mutex<HubState>,
}

impl TaskHub {
    fn begin(&self, request: String) -> Result<TaskSnapshot, String> {
        let mut state = self.state.lock().expect("task hub poisoned");
        if state.active.is_some() {
            return Err("busy: DeskAct is already running a task. It was not queued.".to_owned());
        }

        let snapshot = TaskSnapshot {
            task_id: self.next_task_id.fetch_add(1, Ordering::Relaxed) + 1,
            revision: self.next_revision.fetch_add(1, Ordering::Relaxed) + 1,
            request,
            phase: TaskPhase::Planning,
            message: "Preparing the request and checking the local app catalog.".to_owned(),
            report: None,
            test_fixture: fixture_mode_enabled(),
        };
        state.active = Some(ActiveTask {
            snapshot: snapshot.clone(),
        });
        Ok(snapshot)
    }

    fn update(&self, task_id: u64, update: impl FnOnce(&mut TaskSnapshot)) -> Option<TaskSnapshot> {
        let mut state = self.state.lock().expect("task hub poisoned");
        let active = state.active.as_mut()?;
        if active.snapshot.task_id != task_id {
            return None;
        }
        update(&mut active.snapshot);
        active.snapshot.revision = self.next_revision.fetch_add(1, Ordering::Relaxed) + 1;
        Some(active.snapshot.clone())
    }

    fn finish(&self, task_id: u64, update: impl FnOnce(&mut TaskSnapshot)) -> Option<TaskSnapshot> {
        let mut state = self.state.lock().expect("task hub poisoned");
        if state.active.as_ref()?.snapshot.task_id != task_id {
            return None;
        }
        let mut active = state.active.take()?;
        update(&mut active.snapshot);
        active.snapshot.revision = self.next_revision.fetch_add(1, Ordering::Relaxed) + 1;
        if !active.snapshot.phase.is_terminal() {
            state.active = Some(active);
            return None;
        }
        let snapshot = active.snapshot;
        state.last_result = Some(snapshot.clone());
        Some(snapshot)
    }

    fn snapshot(&self) -> Option<TaskSnapshot> {
        let state = self.state.lock().expect("task hub poisoned");
        state
            .active
            .as_ref()
            .map(|active| active.snapshot.clone())
            .or_else(|| state.last_result.clone())
    }

    fn last_result(&self) -> Option<TaskSnapshot> {
        self.state
            .lock()
            .expect("task hub poisoned")
            .last_result
            .clone()
    }
}

#[tauri::command]
pub fn get_task_snapshot(hub: State<'_, TaskHub>) -> Option<TaskSnapshot> {
    hub.snapshot()
}

#[tauri::command]
pub fn get_last_task_result(hub: State<'_, TaskHub>) -> Option<TaskSnapshot> {
    hub.last_result()
}

#[tauri::command]
pub fn submit_task(
    app: AppHandle,
    hub: State<'_, TaskHub>,
    request: String,
) -> Result<TaskSnapshot, String> {
    let snapshot = hub.begin(request.clone())?;
    emit_snapshot(&app, &snapshot);
    let app_handle = app.clone();
    let task_id = snapshot.task_id;

    tauri::async_runtime::spawn(async move {
        let prepared = match prepare_task_request(&request).await {
            Ok(prepared) => prepared,
            Err(error) => {
                finish_task(&app_handle, task_id, |snapshot| {
                    snapshot.phase = TaskPhase::Failed;
                    snapshot.message = user_message(&error);
                });
                return;
            }
        };

        update_task(&app_handle, task_id, |snapshot| {
            snapshot.phase = TaskPhase::Running;
            snapshot.message =
                "Opening the selected app and checking fresh native evidence.".to_owned();
        });

        let outcome = tauri::async_runtime::spawn_blocking(move || {
            let mut cli = None;
            execute_prepared_task(prepared, |entry| {
                if fixture_mode_enabled() {
                    return Ok(fixture_resolution(entry));
                }
                if cli.is_none() {
                    cli = Some(VerifiedNativeCli::connect()?);
                }
                cli.as_ref()
                    .expect("native CLI initialized")
                    .resolve_target(entry.as_discovered_app())
            })
        })
        .await;

        match outcome {
            Ok(Ok(report)) => {
                let (phase, message) = match report.status.as_str() {
                    "completed" => (TaskPhase::Completed, report.summary.clone()),
                    "partial" => (
                        TaskPhase::Partial,
                        "Some work completed, but one or more outcomes remain unverified."
                            .to_owned(),
                    ),
                    _ => (
                        TaskPhase::Failed,
                        "DeskAct could not verify the requested app-opening task.".to_owned(),
                    ),
                };
                finish_task(&app_handle, task_id, |snapshot| {
                    snapshot.phase = phase;
                    snapshot.message = message;
                    snapshot.report = Some(report);
                });
            }
            Ok(Err(error)) => {
                finish_task(&app_handle, task_id, |snapshot| {
                    snapshot.phase = TaskPhase::Failed;
                    snapshot.message = user_message(&error);
                });
            }
            Err(_) => {
                finish_task(&app_handle, task_id, |snapshot| {
                    snapshot.phase = TaskPhase::Failed;
                    snapshot.message =
                        "DeskAct's background task did not complete. Check the app and try again."
                            .to_owned();
                });
            }
        }
    });

    Ok(snapshot)
}

fn update_task(app: &AppHandle, task_id: u64, update: impl FnOnce(&mut TaskSnapshot)) {
    if let Some(snapshot) = app.state::<TaskHub>().update(task_id, update) {
        emit_snapshot(app, &snapshot);
    }
}

fn finish_task(app: &AppHandle, task_id: u64, update: impl FnOnce(&mut TaskSnapshot)) {
    if let Some(snapshot) = app.state::<TaskHub>().finish(task_id, update) {
        emit_snapshot(app, &snapshot);
    }
}

fn emit_snapshot(app: &AppHandle, snapshot: &TaskSnapshot) {
    let _ = app.emit_to("main", TASK_UPDATED_EVENT, snapshot);
}

fn user_message(error: &DeskActError) -> String {
    error.to_string()
}

#[cfg(debug_assertions)]
fn fixture_mode_enabled() -> bool {
    std::env::var("DESKACT_HUD_TEST_FIXTURE").is_ok_and(|value| value == "1")
}

#[cfg(not(debug_assertions))]
fn fixture_mode_enabled() -> bool {
    false
}

#[cfg(debug_assertions)]
fn fixture_resolution(entry: &deskact_core::AppCatalogEntry) -> NativeTargetResolution {
    use deskact_core::{NativeWindow, ResolvedNativeTarget};

    NativeTargetResolution::Ready(ResolvedNativeTarget {
        app: entry.as_discovered_app(),
        window: NativeWindow {
            id: "fixture-window".to_owned(),
            title: entry.name.clone(),
            app_name: entry.name.clone(),
            accessible: true,
            minimized: false,
            is_focused: false,
            visible: true,
            pid: 0,
        },
        snapshot_id: "fixture-snapshot".to_owned(),
        ref_count: 1,
    })
}

#[cfg(not(debug_assertions))]
fn fixture_resolution(_entry: &deskact_core::AppCatalogEntry) -> NativeTargetResolution {
    unreachable!("fixture resolver is disabled in release builds")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_submission_is_refused_without_queueing() {
        let hub = TaskHub::default();
        let first = hub.begin("open Calculator".to_owned()).expect("start task");
        let second = hub.begin("open Notepad".to_owned());

        assert_eq!(first.phase, TaskPhase::Planning);
        assert!(matches!(second, Err(error) if error.contains("not queued")));
        assert_eq!(
            hub.snapshot().expect("active snapshot").task_id,
            first.task_id
        );
    }

    #[test]
    fn completed_task_becomes_the_session_result_and_a_new_task_can_start() {
        let hub = TaskHub::default();
        let first = hub
            .begin("open Calculator".to_owned())
            .expect("start first task");
        let completed = hub
            .finish(first.task_id, |snapshot| {
                snapshot.phase = TaskPhase::Completed;
                snapshot.message = "Calculator is open".to_owned();
            })
            .expect("finish active task");
        let second = hub
            .begin("open Notepad".to_owned())
            .expect("start second task");

        assert_eq!(
            hub.snapshot().expect("active snapshot").task_id,
            second.task_id
        );
        assert_eq!(hub.last_result().expect("session result"), completed);
        assert_eq!(completed.phase, TaskPhase::Completed);
    }

    #[test]
    fn only_the_active_task_can_update_or_finish_the_snapshot() {
        let hub = TaskHub::default();
        let active = hub.begin("open Calculator".to_owned()).expect("start task");

        assert!(hub.update(active.task_id + 1, |_| {}).is_none());
        assert!(
            hub.finish(active.task_id + 1, |snapshot| {
                snapshot.phase = TaskPhase::Completed;
            })
            .is_none()
        );
        assert_eq!(
            hub.snapshot().expect("active task persists").task_id,
            active.task_id
        );
    }
}
