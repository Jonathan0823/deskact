import { useEffect, useLayoutEffect, useRef, useState, type FormEvent } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { Play } from "lucide-react";

type PrototypeState = "composer" | "working" | "attention" | "settings";
type PreviewState = "hidden" | "command" | "working" | "details" | "approval" | "done" | "error" | "cancelled";
const previewStates: Array<{ id: PreviewState; label: string }> = [
  { id: "hidden", label: "Hidden" },
  { id: "command", label: "Command" },
  { id: "working", label: "Working" },
  { id: "details", label: "Details" },
  { id: "approval", label: "Approval" },
  { id: "done", label: "Done" },
  { id: "error", label: "Error" },
  { id: "cancelled", label: "Stopped" },
];
type TaskPhase = "planning" | "running" | "completed" | "partial" | "failed" | "cancelled";
type TaskReport = {
  summary: string;
  status: string;
  achieved_postconditions: string[];
  unresolved_steps: string[];
  evidence: Array<{ app_name: string }>;
};
type TaskSnapshot = {
  task_id: number;
  revision: number;
  request: string;
  phase: TaskPhase;
  message: string;
  report: TaskReport | null;
  test_fixture: boolean;
  cancellation_requested: boolean;
};
type ShortcutStatus = {
  accelerator: string;
  registered: boolean;
  error: string | null;
};

const defaultShortcut: ShortcutStatus = {
  accelerator: "Ctrl+Shift+Space",
  registered: false,
  error: null,
};

function isActiveTask(task: TaskSnapshot | null) {
  return task?.phase === "planning" || task?.phase === "running";
}

function isNewerTask(incoming: TaskSnapshot, current: TaskSnapshot | null) {
  return !current || incoming.task_id > current.task_id ||
    (incoming.task_id === current.task_id && incoming.revision > current.revision);
}

function phaseLabel(phase: TaskPhase) {
  switch (phase) {
    case "planning": return "Preparing task";
    case "running": return "Task in progress";
    case "completed": return "Completed and verified";
    case "partial": return "Partially completed";
    case "failed": return "Could not verify completion";
    case "cancelled": return "Stopped";
  }
}

function previewTask(state: PreviewState): TaskSnapshot | null {
  const phase: TaskPhase | null = state === "working" || state === "details" ? "running"
    : state === "done" ? "completed"
      : state === "error" ? "failed"
        : state === "cancelled" ? "cancelled"
          : null;
  if (!phase) return null;

  const message = state === "details" ? "Finding Liked Songs"
    : state === "working" ? "Opening Spotify"
      : state === "done" ? "Playback verified"
        : state === "error" ? "Couldn't identify the right window"
          : "Task stopped";
  return {
    task_id: 1,
    revision: 1,
    request: "Open Spotify and play my Liked Songs",
    phase,
    message,
    report: null,
    test_fixture: true,
    cancellation_requested: false,
  };
}

export default function App() {
  const [state, setState] = useState<PrototypeState>("composer");
  const [previewState, setPreviewState] = useState<PreviewState>("command");
  const [shortcut, setShortcut] = useState<ShortcutStatus>(defaultShortcut);
  const [shortcutInput, setShortcutInput] = useState(defaultShortcut.accelerator);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [request, setRequest] = useState("");
  const [task, setTask] = useState<TaskSnapshot | null>(null);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [detailsExpanded, setDetailsExpanded] = useState(false);
  const [quitRequested, setQuitRequested] = useState(false);
  const requestInput = useRef<HTMLInputElement>(null);
  const taskRef = useRef<TaskSnapshot | null>(null);
  const browserPreview = import.meta.env.DEV && !isTauri();
  const previewViewState: PrototypeState | "hidden" = previewState === "hidden" ? "hidden"
    : previewState === "command" ? "composer"
      : previewState === "approval" ? "attention"
        : "working";
  const displayedState = browserPreview ? previewViewState : state;
  const displayedTask = browserPreview ? previewTask(previewState) : task;
  const displayedDetailsExpanded = browserPreview ? previewState === "details" : detailsExpanded;

  useLayoutEffect(() => {
    if (!isTauri()) return;

    let dimensions: [number, number];
    if (quitRequested) dimensions = [490, 220];
    else if (state === "composer") dimensions = [506, 88];
    else if (state === "settings") dimensions = [520, 300];
    else if (state === "attention") dimensions = [490, 200];
    else if (detailsExpanded) dimensions = [502, 200];
    else if (task?.phase === "partial" || task?.phase === "failed") dimensions = [366, 64];
    else dimensions = [task?.phase === "completed" || task?.phase === "cancelled" ? 296 : 352, 56];

    void getCurrentWindow().setSize(new LogicalSize(...dimensions)).catch(() => {});
  }, [state, quitRequested, detailsExpanded, task?.phase]);

  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    const listeners: Array<() => void> = [];

    void listen("deskact://invoked", () => { void revealDeskAct(); }).then((unlisten) => {
      if (disposed) unlisten();
      else listeners.push(unlisten);
    });
    void listen("deskact://open-settings", () => {
      setState("settings");
      void refreshShortcut();
    }).then((unlisten) => {
      if (disposed) unlisten();
      else listeners.push(unlisten);
    });
    void listen("deskact://quit-requested", () => setQuitRequested(true)).then((unlisten) => {
      if (disposed) unlisten();
      else listeners.push(unlisten);
    });
    void listen<TaskSnapshot>("deskact://task-updated", (event) => {
      const incoming = event.payload;
      if (acceptTaskSnapshot(incoming)) {
        if (isActiveTask(incoming)) setState("working");
        else setQuitRequested(false);
      }
    }).then((unlisten) => {
      if (disposed) unlisten();
      else listeners.push(unlisten);
    });
    void refreshCurrentTask();

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        void invoke("dismiss_island");
      }
    };
    window.addEventListener("keydown", onKeyDown);

    return () => {
      disposed = true;
      listeners.forEach((unlisten) => unlisten());
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);

  function acceptTaskSnapshot(incoming: TaskSnapshot) {
    if (!isNewerTask(incoming, taskRef.current)) return false;
    taskRef.current = incoming;
    setTask(incoming);
    return true;
  }

  async function refreshCurrentTask() {
    if (!isTauri()) return null;
    try {
      const current = await invoke<TaskSnapshot | null>("get_task_snapshot");
      if (current) {
        acceptTaskSnapshot(current);
        if (isActiveTask(current)) setState("working");
      }
      return current;
    } catch {
      return null;
    }
  }

  async function revealDeskAct() {
    const current = await refreshCurrentTask();
    if (isActiveTask(current)) {
      setState("working");
      return;
    }
    setState("composer");
    requestAnimationFrame(() => requestInput.current?.focus());
  }

  async function refreshShortcut() {
    if (!isTauri()) return;
    try {
      const current = await invoke<ShortcutStatus>("get_shortcut_status");
      setShortcut(current);
      setShortcutInput(current.accelerator);
      setSaveError(null);
    } catch {
      setSaveError("Could not read shortcut settings. Tray access remains available.");
    }
  }

  async function saveShortcut() {
    setSaveError(null);
    if (!isTauri()) {
      setSaveError("Shortcut settings are available in the Windows app.");
      return;
    }
    try {
      const updated = await invoke<ShortcutStatus>("update_global_shortcut", {
        accelerator: shortcutInput,
      });
      setShortcut(updated);
      setShortcutInput(updated.accelerator);
    } catch (error) {
      await refreshShortcut();
      setSaveError(String(error));
    }
  }

  async function stopTask() {
    if (!task || !isActiveTask(task)) return;
    try {
      await invoke("stop_task", { taskId: task.task_id });
    } catch (error) {
      setSubmitError(String(error));
    }
  }

  async function submitRequest(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const submitted = request.trim();
    if (!submitted || !isTauri()) return;
    setSubmitError(null);
    setDetailsExpanded(false);
    try {
      const started = await invoke<TaskSnapshot>("submit_task", { request: submitted });
      acceptTaskSnapshot(started);
      setRequest("");
      setState("working");
      void invoke("yield_focus");
    } catch (error) {
      const current = await refreshCurrentTask();
      setSubmitError(String(error).startsWith("busy:")
        ? "DeskAct is busy with the task shown below. Requests are not queued; resubmit after it finishes."
        : String(error));
      if (isActiveTask(current)) setState("working");
    }
  }

  const visualState = quitRequested ? "attention" : displayedState === "working"
    ? displayedTask?.phase === "completed" ? "done"
      : displayedTask?.phase === "partial" || displayedTask?.phase === "failed" ? "error"
        : displayedTask?.phase === "cancelled" ? "cancelled"
          : "working"
    : displayedState;

  return (
    <div className={browserPreview ? "web-preview" : undefined}>
      {displayedState !== "hidden" && <main className={`island state-${visualState}${displayedDetailsExpanded ? " expanded" : ""}`} aria-label="DeskAct Adaptive Island">
      {displayedState === "composer" && (
        <>
          <section className="shell composer-shell">
            <form onSubmit={(event) => void submitRequest(event)}>
              <label className="sr-only" htmlFor="request">What would you like DeskAct to do?</label>
              <div className="input-row">
                <input
                  ref={requestInput}
                  id="request"
                  className="command-input"
                  value={request}
                  onChange={(event) => setRequest(event.target.value)}
                  placeholder="Ask DeskAct to do something…"
                  autoComplete="off"
                  spellCheck={false}
                />
                <button className="play-button" type="submit" aria-label="Run command" disabled={!request.trim()}>
                  <Play size={15} fill="currentColor" strokeWidth={1.5} aria-hidden="true" />
                </button>
              </div>
              <div className="composer-hints">
                <span>Enter to run</span>
                <span>Esc to dismiss</span>
              </div>
            </form>
            {submitError && <p className="error-message" role="alert">{submitError}</p>}
          </section>
        </>
      )}

      {displayedState === "working" && (
        displayedTask ? (
          <TaskStatus
            task={displayedTask}
            detailsExpanded={displayedDetailsExpanded}
            onToggleDetails={() => browserPreview ? setPreviewState(displayedDetailsExpanded ? "working" : "details") : setDetailsExpanded((value) => !value)}
            onStop={() => browserPreview ? setPreviewState("cancelled") : void stopTask()}
          />
        ) : (
          <section className="shell compact-shell" aria-label="Task status">
            <div className="status-copy">
              <div className="status-title">Preparing task</div>
              <div className="status-sub">Checking the request</div>
            </div>
          </section>
        )
      )}

      {displayedState === "attention" && (
        <section className="shell attention-shell" aria-labelledby="attention-title">
          <p className="attention-label" id="attention-title">Approval required</p>
          <p className="approval-text">Move 3 selected files to the Recycle Bin?</p>
          <div className="resource">Affected location · Downloads</div>
          <div className="button-row">
            <button className="quiet-button" type="button" onClick={() => browserPreview ? setPreviewState("cancelled") : setState("working")}>Reject</button>
            <button className="primary-button" type="button" onClick={() => browserPreview ? setPreviewState("done") : setState("working")}>Approve</button>
          </div>
          {browserPreview && <p className="preview-only">Preview only · no files will change</p>}
        </section>
      )}

      {quitRequested && (
        <section className="quit-dialog" role="alertdialog" aria-labelledby="quit-title" aria-describedby="quit-description">
          <h1 id="quit-title">Quit while a task is active?</h1>
          <p id="quit-description">DeskAct will stop running. Any in-flight app opening may still complete, and its outcome may be uncertain. Quit does not roll back completed effects.</p>
          <div className="button-row">
            <button className="quiet-button" type="button" onClick={() => setQuitRequested(false)}>Keep DeskAct open</button>
            <button className="danger-button" type="button" onClick={() => void invoke("quit_application")}>Quit anyway</button>
          </div>
        </section>
      )}

      {!browserPreview && displayedState === "settings" && (
        <section className="surface settings-surface" aria-labelledby="settings-title">
          <h1 id="settings-title">Settings</h1>
          <p className="settings-copy">Global shortcut opens DeskAct from another application. Tray access remains available if registration fails.</p>
          <label htmlFor="shortcut">Global shortcut</label>
          <input
            id="shortcut"
            value={shortcutInput}
            onChange={(event) => setShortcutInput(event.target.value)}
            placeholder="Ctrl+Shift+Space"
            spellCheck={false}
          />
          <p className={shortcut.registered ? "shortcut-status" : "shortcut-status warning"} role="status">
            {shortcut.registered ? `Registered: ${shortcut.accelerator}` : "Shortcut is not registered; use the tray to reopen DeskAct."}
          </p>
          {(shortcut.error || saveError) && <p className="error-message" role="alert">{saveError ?? shortcut.error}</p>}
          <div className="button-row">
            <button className="quiet-button" type="button" onClick={() => setState("composer")}>Back</button>
            <button className="primary-button" type="button" onClick={() => void saveShortcut()}>Save shortcut</button>
          </div>
        </section>
      )}
      </main>}
      {browserPreview && <PreviewControls selected={previewState} onSelect={setPreviewState} />}
    </div>
  );
}

export function TaskStatus({
  task,
  detailsExpanded,
  onToggleDetails,
  onStop,
}: {
  task: TaskSnapshot;
  detailsExpanded: boolean;
  onToggleDetails: () => void;
  onStop: () => void;
}) {
  const active = isActiveTask(task);
  return (
    <section className="shell compact-shell" aria-label="Task status">
      <button className="status-trigger" type="button" aria-expanded={detailsExpanded} onClick={onToggleDetails}>
        <span className="status-title">{task.message || phaseLabel(task.phase)}</span>
        <span className="status-sub">{task.test_fixture ? "Preview only · no action taken" : phaseLabel(task.phase)}</span>
      </button>
      {active && (
        <button className="text-control stop-control" type="button" disabled={task.cancellation_requested} onClick={onStop}>
          {task.cancellation_requested ? "Stopping" : "Stop"}
        </button>
      )}
      <button className="text-control detail-control" type="button" aria-expanded={detailsExpanded} onClick={onToggleDetails}>
        {detailsExpanded ? "Hide" : "Details"}
      </button>
      {detailsExpanded && (
        <div className="task-detail-list">
          <span className="mini-label">{task.test_fixture ? "Preview details" : task.report?.status === "completed" ? "Verified result" : "Task details"}</span>
          {task.test_fixture
            ? <><p>Open Spotify · Find Liked Songs · Start playback</p><p>Preview only; no desktop action was dispatched or verified.</p></>
            : task.report
              ? <TaskReportDetails report={task.report} testFixture={false} />
              : <p>{task.message || "DeskAct is checking the request and supported target."}</p>}
        </div>
      )}
    </section>
  );
}

function PreviewControls({ selected, onSelect }: { selected: PreviewState; onSelect: (state: PreviewState) => void }) {
  return (
    <aside className="preview-controls" aria-label="Prototype controls">
      <div className="preview-controls-heading">
        <strong>Prototype controls</strong>
        <span>v0.1</span>
      </div>
      <div className="preview-state-grid">
        {previewStates.map(({ id, label }) => (
          <button key={id} className={selected === id ? "preview-state selected" : "preview-state"} type="button" aria-pressed={selected === id} onClick={() => onSelect(id)}>
            {label}
          </button>
        ))}
      </div>
      <p className="preview-controls-note">Preview only · no desktop actions run here</p>
    </aside>
  );
}

function TaskReportDetails({ report, testFixture }: { report: TaskReport; testFixture: boolean }) {
  return (
    <div className="task-detail-list">
      <strong>{testFixture ? "Fixture-only outcome" : report.status === "completed" ? "Verified results" : "Outcome details"}</strong>
      {testFixture ? (
        <p>Fixture data only; no native action or verification was performed.</p>
      ) : (
        <>
          {report.evidence.map((item) => <p key={item.app_name}>{item.app_name}: open in a uniquely matched accessible window with a fresh snapshot.</p>)}
          {report.achieved_postconditions.map((item) => <p key={item}>{item}</p>)}
          {report.unresolved_steps.map((item) => <p className="warning-copy" key={item}>{item}</p>)}
          {!report.evidence.length && !report.unresolved_steps.length && <p>{report.summary}</p>}
        </>
      )}
    </div>
  );
}
