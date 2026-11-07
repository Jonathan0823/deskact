import { useEffect, useLayoutEffect, useRef, useState, type FormEvent } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";

type PrototypeState = "composer" | "working" | "attention" | "settings";
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

const stateLabels: Record<Exclude<PrototypeState, "settings">, string> = {
  composer: "Composer",
  working: "Passive status",
  attention: "Attention card",
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

export default function App() {
  const [state, setState] = useState<PrototypeState>("composer");
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
  const showPrototypeStates = import.meta.env.DEV || import.meta.env.VITE_PROTOTYPE_PREVIEWS === "true";

  useLayoutEffect(() => {
    if (!isTauri()) return;

    const dimensions: Record<PrototypeState, [number, number]> = {
      composer: [600, 180],
      working: [460, 158],
      attention: [600, 238],
      settings: [520, 300],
    };
    const [width, height] = quitRequested ? [460, 230] : dimensions[state];
    void getCurrentWindow().setSize(new LogicalSize(width, height)).catch(() => {});
  }, [state, quitRequested]);

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

  return (
    <main className="island" aria-label="DeskAct Adaptive Island">
      <header className="island-header">
        <span className="brand-mark" aria-hidden="true">D</span>
        <span className="brand-name">DeskAct</span>
        <span className="prototype-tag">WINDOW PROTOTYPE</span>
        <button
          className="icon-button"
          type="button"
          aria-label="Hide DeskAct"
          title="Hide DeskAct"
          onClick={() => { if (isTauri()) void invoke("dismiss_island"); }}
        >
          <span aria-hidden="true">−</span>
        </button>
      </header>

      {showPrototypeStates && state !== "settings" && (
        <nav className="prototype-nav" aria-label="Prototype states">
          {(Object.keys(stateLabels) as Array<Exclude<PrototypeState, "settings">>).map((value) => (
            <button
              className={`state-button ${state === value ? "selected" : ""}`}
              key={value}
              type="button"
              aria-pressed={state === value}
              onClick={() => setState(value)}
            >
              {stateLabels[value]}
            </button>
          ))}
        </nav>
      )}

      {state === "composer" && (
        <>
          <form className="surface composer-surface" aria-labelledby="composer-title" onSubmit={(event) => void submitRequest(event)}>
            <label id="composer-title" className="sr-only" htmlFor="request">What would you like DeskAct to do?</label>
            <input
              ref={requestInput}
              id="request"
              value={request}
              onChange={(event) => setRequest(event.target.value)}
              placeholder="What would you like DeskAct to do?"
            />
            <button className="submit-button" type="submit" aria-label="Submit request" title="Submit request" disabled={!request.trim()}>
              <span aria-hidden="true">↑</span>
            </button>
            {!isTauri() && <p className="hint">Prototype only · requests are not executed in browser preview</p>}
          </form>
          {submitError && <p className="error-message submit-error" role="alert">{submitError}</p>}
          {task && !isActiveTask(task) && <TaskResult task={task} />}
        </>
      )}

      {state === "working" && (
        task ? (
          <TaskStatus
            task={task}
            detailsExpanded={detailsExpanded}
            onToggleDetails={() => setDetailsExpanded((value) => !value)}
            onStop={() => void stopTask()}
          />
        ) : (
          <section className="surface status-surface" aria-label="Simulated task status">
            <span className="activity-dot" aria-hidden="true" />
            <div className="status-copy">
              <strong>Simulated task in progress</strong>
              <span>No desktop action is running.</span>
            </div>
            <button className="quiet-button" type="button" aria-label="Expand simulated task details">Details</button>
          </section>
        )
      )}

      {state === "attention" && (
        <section className="surface attention-surface" aria-labelledby="attention-title">
          <div className="attention-heading">
            <span className="attention-icon" aria-hidden="true">!</span>
            <div>
              <h1 id="attention-title">Window behavior needs validation</h1>
              <p>This prototype does not request approval or control another app.</p>
            </div>
          </div>
          <div className="button-row">
            <button className="quiet-button" type="button" onClick={() => setState("working")}>Show status preview</button>
            <button className="primary-button" type="button" onClick={() => setState("composer")}>Return to composer</button>
          </div>
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

      {state === "settings" && (
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
    </main>
  );
}

function TaskStatus({
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
  return (
    <section className="surface task-surface" aria-label="Task status">
      <span className={`activity-dot ${isActiveTask(task) ? "" : "terminal"}`} aria-hidden="true" />
      <div className="status-copy">
        <strong>{task.test_fixture ? "Test fixture · simulated outcome" : phaseLabel(task.phase)}</strong>
        <span>{task.test_fixture ? "No desktop action was dispatched." : task.message}</span>
        {task.test_fixture && <span className="fixture-tag">TEST FIXTURE · result is not native verification</span>}
      </div>
      {isActiveTask(task) && (
        <button className="danger-button stop-button" type="button" disabled={task.cancellation_requested} onClick={onStop}>
          {task.cancellation_requested ? "Stopping…" : "Stop"}
        </button>
      )}
      {(task.report || task.phase === "planning" || task.phase === "running") && (
        <button className="quiet-button" type="button" aria-expanded={detailsExpanded} onClick={onToggleDetails}>
          {detailsExpanded ? "Hide details" : "Details"}
        </button>
      )}
      {detailsExpanded && task.report && <TaskReportDetails report={task.report} testFixture={task.test_fixture} />}
      {detailsExpanded && !task.report && <p className="task-detail">DeskAct is checking the request and its supported app target.</p>}
    </section>
  );
}

function TaskResult({ task }: { task: TaskSnapshot }) {
  return (
    <section className="surface last-result" aria-label="Last task result">
      <span className="result-label">{task.test_fixture ? "TEST FIXTURE · SIMULATED" : `LAST TASK · ${phaseLabel(task.phase)}`}</span>
      <p>{task.test_fixture ? "No desktop action was dispatched." : task.message}</p>
    </section>
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
