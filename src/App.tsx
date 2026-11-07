import { useEffect, useLayoutEffect, useRef, useState, type FormEvent } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Check, LoaderCircle, Mic, Play, TriangleAlert, X } from "lucide-react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";

type PrototypeState = "composer" | "working" | "attention" | "settings" | "voice";
type PreviewState = "hidden" | "command" | "voice" | "working" | "details" | "approval" | "destructive" | "removed" | "destroyed" | "done" | "error" | "cancelled";
const previewStates: Array<{ id: PreviewState; label: string }> = [
  { id: "hidden", label: "Hidden" },
  { id: "command", label: "Command" },
  { id: "voice", label: "Voice" },
  { id: "working", label: "Working" },
  { id: "details", label: "Details" },
  { id: "approval", label: "Remove" },
  { id: "destructive", label: "Destruction" },
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
type TaskProgress = {
  app_name: string;
  current_step: number;
  total_steps: number;
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
  progress?: TaskProgress | null;
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

function previewView(state: PreviewState): PrototypeState | "hidden" {
  switch (state) {
    case "hidden": return "hidden";
    case "command": return "composer";
    case "approval":
    case "destructive": return "attention";
    case "voice": return "voice";
    default: return "working";
  }
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

const demoSteps = ["Opening Spotify", "Finding Liked Songs", "Starting playback"];
const demoVoiceCommand = "Open Spotify and play my Liked Songs";
function previewTask(state: PreviewState, step: number): TaskSnapshot | null {
  const phase: TaskPhase | null = state === "working" || state === "details" ? "running"
    : state === "done" || state === "removed" || state === "destroyed" ? "completed"
      : state === "error" ? "failed"
        : state === "cancelled" ? "cancelled"
          : null;
  if (!phase) return null;

  const currentStep = state === "done" ? demoSteps.length - 1 : Math.max(0, Math.min(step, demoSteps.length - 1));
  const message = state === "done" ? "Playback verified"
    : state === "removed" ? "Moved to Recycle Bin"
      : state === "destroyed" ? "Permanently deleted"
        : state === "error" ? "Couldn't identify the right window"
      : state === "cancelled" ? "Task stopped"
        : demoSteps[currentStep];
  return {
    task_id: 1,
    revision: currentStep + 1,
    request: "Open Spotify and play my Liked Songs",
    phase,
    message,
    report: null,
    test_fixture: true,
    cancellation_requested: false,
    progress: state === "removed" || state === "destroyed" ? null : { app_name: "Spotify", current_step: currentStep + 1, total_steps: demoSteps.length },
  };
}

export default function App() {
  const [state, setState] = useState<PrototypeState>("composer");
  const [previewState, setPreviewState] = useState<PreviewState>("command");
  const [previewStep, setPreviewStep] = useState(0);
  const [streamedTranscript, setStreamedTranscript] = useState("");
  const [islandVisible, setIslandVisible] = useState(() => !isTauri());
  const [shortcut, setShortcut] = useState<ShortcutStatus>(defaultShortcut);
  const [shortcutInput, setShortcutInput] = useState(defaultShortcut.accelerator);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [request, setRequest] = useState("");
  const [task, setTask] = useState<TaskSnapshot | null>(null);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [detailsExpanded, setDetailsExpanded] = useState(false);
  const [quitRequested, setQuitRequested] = useState(false);
  const requestInput = useRef<HTMLInputElement>(null);
  const islandRef = useRef<HTMLElement>(null);
  const taskRef = useRef<TaskSnapshot | null>(null);
  const hideTimer = useRef<number | null>(null);
  const browserPreview = import.meta.env.DEV && !isTauri();
  const prefersReducedMotion = useReducedMotion();
  const displayedState = browserPreview ? previewView(previewState) : islandVisible ? state : "hidden";
  const displayedTask = browserPreview ? previewTask(previewState, previewStep) : task;
  const displayedDetailsExpanded = browserPreview ? previewState === "details" : detailsExpanded;
  const lastPreviewState = useRef<PreviewState>("command");
  if (browserPreview && previewState !== "hidden") lastPreviewState.current = previewState;
  const contentState = browserPreview && previewState === "hidden"
    ? previewView(lastPreviewState.current)
    : displayedState === "hidden" ? state : displayedState;
  const contentTask = browserPreview && previewState === "hidden"
    ? previewTask(lastPreviewState.current, previewStep)
    : displayedTask;
  const contentDetailsExpanded = browserPreview && previewState === "hidden"
    ? lastPreviewState.current === "details"
    : displayedDetailsExpanded;
  const destructiveApproval = browserPreview && (previewState === "destructive" || (previewState === "hidden" && lastPreviewState.current === "destructive"));

  function selectPreviewState(next: PreviewState) {
    if (next === "working") setPreviewStep(0);
    if (next === "voice") setStreamedTranscript("");
    setPreviewState(next);
  }

  function dismissIsland() {
    if (browserPreview) {
      setPreviewState("hidden");
      return;
    }
    setIslandVisible(false);
    if (hideTimer.current !== null) window.clearTimeout(hideTimer.current);
    const delay = window.matchMedia("(prefers-reduced-motion: reduce)").matches ? 0 : 240;
    hideTimer.current = window.setTimeout(() => {
      void invoke("dismiss_island");
      hideTimer.current = null;
    }, delay);
  }

  useLayoutEffect(() => {
    if (!isTauri()) return;

    let width: number;
    if (quitRequested || state === "attention") width = 490;
    else if (state === "composer") width = 506;
    else if (state === "settings") width = 520;
    else if (state === "voice") width = 360;
    else if (detailsExpanded) width = 502;
    else if (task?.phase === "partial" || task?.phase === "failed") width = 366;
    else width = task?.phase === "completed" || task?.phase === "cancelled" ? 296 : 352;
    const height = Math.ceil(islandRef.current?.scrollHeight ?? 88);

    const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    void invoke("morph_island_window", {
      width,
      height,
      reducedMotion,
    }).catch(() => {});
  }, [state, quitRequested, detailsExpanded, task?.phase]);

  useEffect(() => {
    if (!browserPreview || previewState !== "working") return;
    let currentStep = 0;
    const timer = window.setInterval(() => {
      currentStep += 1;
      if (currentStep >= demoSteps.length) {
        window.clearInterval(timer);
        setPreviewState("done");
        return;
      }
      setPreviewStep(currentStep);
    }, 2050);
    return () => window.clearInterval(timer);
  }, [browserPreview, previewState]);

  useEffect(() => {
    if (!browserPreview || previewState !== "voice") return;
    const words = demoVoiceCommand.match(/\S+\s*/g) ?? [demoVoiceCommand];
    let visibleWords = 0;
    let completionTimer: number | null = null;
    const timer = window.setInterval(() => {
      visibleWords = Math.min(words.length, visibleWords + 1);
      setStreamedTranscript(words.slice(0, visibleWords).join(""));
      if (visibleWords >= words.length) {
        window.clearInterval(timer);
        setRequest(demoVoiceCommand);
        completionTimer = window.setTimeout(() => setPreviewState("command"), 800);
      }
    }, 120);
    return () => {
      window.clearInterval(timer);
      if (completionTimer !== null) window.clearTimeout(completionTimer);
    };
  }, [browserPreview, previewState]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        dismissIsland();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    if (!isTauri()) return () => window.removeEventListener("keydown", onKeyDown);

    let disposed = false;
    const listeners: Array<() => void> = [];

    void listen("deskact://invoked", () => { void revealDeskAct(); }).then((unlisten) => {
      if (disposed) unlisten();
      else listeners.push(unlisten);
    });
    void listen("deskact://open-settings", () => {
      setIslandVisible(true);
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
    if (hideTimer.current !== null) {
      window.clearTimeout(hideTimer.current);
      hideTimer.current = null;
    }
    setIslandVisible(true);
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

  const visualState = quitRequested ? "attention" : contentState === "working"
    ? contentTask?.phase === "completed" ? "done"
      : contentTask?.phase === "partial" || contentTask?.phase === "failed" ? "error"
        : contentTask?.phase === "cancelled" ? "cancelled"
          : "working"
    : contentState;

  return (
    <div className={browserPreview ? "web-preview" : "tauri-root"}>
      <main ref={islandRef} className={`island state-${visualState}${contentDetailsExpanded ? " expanded" : ""}${quitRequested ? " quit-confirming" : ""}${displayedState === "hidden" ? " is-hidden" : ""}`} aria-label="DeskAct Adaptive Island" aria-hidden={displayedState === "hidden"} inert={displayedState === "hidden"}>
      <AnimatePresence mode="popLayout" initial={false}>
      <motion.div
        key={contentState}
        className={`island-content state-content-${contentState}`}
        initial={prefersReducedMotion ? { opacity: 0 } : { opacity: 0, y: 5 }}
        animate={{ opacity: 1, y: 0 }}
        exit={prefersReducedMotion ? { opacity: 0 } : { opacity: 0, y: -4 }}
        transition={{ duration: prefersReducedMotion ? 0.01 : 0.18, ease: [0.22, 1, 0.36, 1] }}
      >
      {contentState === "composer" && (
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
                <div className="composer-voice-entry">
                  {browserPreview && <button className="voice-button" type="button" aria-label="Voice input" onClick={() => selectPreviewState("voice")}><Mic size={15} aria-hidden="true" /></button>}
                </div>
                <div className="composer-key-hints"><span>Enter to run</span><span>Esc to dismiss</span></div>
              </div>
            </form>
            {submitError && <p className="error-message" role="alert">{submitError}</p>}
          </section>
        </>
      )}

      {contentState === "voice" && (
        <section className="shell compact-shell voice-shell" aria-label="Voice input">
          <div className="compact-icon mic-active"><Mic size={17} aria-hidden="true" /></div>
          <div className="voice-copy">
            <div className="voice-live-row">
              <div className="wave" aria-hidden="true">{Array.from({ length: 17 }, (_, index) => <i key={index} />)}</div>
            </div>
            <StreamingText text={streamedTranscript} />
            <span className="sr-only" role="status">Listening for voice input.</span>
          </div>
          <button className="text-control" type="button" onClick={() => selectPreviewState("command")}>Cancel</button>
        </section>
      )}

      {contentState === "working" && (
        contentTask ? (
          <TaskStatus
            task={contentTask}
            detailsExpanded={contentDetailsExpanded}
            onToggleDetails={() => browserPreview ? selectPreviewState(contentDetailsExpanded ? "working" : "details") : setDetailsExpanded((value) => !value)}
            onStop={() => browserPreview ? selectPreviewState("cancelled") : void stopTask()}
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

      {contentState === "attention" && (
        <section className={`shell attention-shell approval-shell ${destructiveApproval ? "approval-destructive" : "approval-reversible"}`} aria-labelledby="attention-title">
          <div className="approval-header">
            <div className={`compact-icon ${destructiveApproval ? "danger" : "warn"}`} aria-hidden="true"><TriangleAlert size={18} /></div>
            <div>
              <p className="attention-label" id="attention-title">{destructiveApproval ? "Permanent deletion" : "Reversible removal"}</p>
              <p className="status-sub">{destructiveApproval ? "Cannot be undone" : "Can be restored from the Recycle Bin"}</p>
            </div>
          </div>
          <p className="approval-text">{destructiveApproval ? <>Permanently delete <strong>3 selected files</strong>?</> : <>Move <strong>3 selected files</strong> to the Recycle Bin?</>}</p>
          <div className="resource"><span className="mini-label">Affected location</span><br />C:\Users\You\Downloads</div>
          <div className="button-row">
            <button className="quiet-button" type="button" onClick={() => browserPreview ? selectPreviewState("cancelled") : setState("working")}>Cancel</button>
            <button className={`primary-button approval-action ${destructiveApproval ? "destructive-action" : "reversible-action"}`} type="button" onClick={() => browserPreview ? selectPreviewState(destructiveApproval ? "destroyed" : "removed") : setState("working")}>
              {destructiveApproval ? "Delete permanently" : "Move to Recycle Bin"}
            </button>
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

      {!browserPreview && contentState === "settings" && (
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
      </motion.div>
      </AnimatePresence>
      </main>
      {browserPreview && <PreviewControls selected={previewState} onSelect={selectPreviewState} />}
    </div>
  );
}

function StreamingText({ text }: { text: string }) {
  const reducedMotion = useReducedMotion();
  const words = text.match(/\S+\s*/g) ?? [];
  return (
    <div className="transcript-line" aria-hidden="true">
      {words.map((word, index) => (
        <motion.span
          key={index}
          className="transcript-word"
          initial={reducedMotion ? { opacity: 0 } : { opacity: 0, filter: "blur(5px)", y: 3 }}
          animate={{ opacity: 1, filter: "blur(0px)", y: 0 }}
          transition={{ duration: reducedMotion ? 0.01 : 0.2, ease: [0.22, 1, 0.36, 1] }}
        >
          {word}
        </motion.span>
      ))}
      <span className="transcript-caret" />
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
  const reducedMotion = useReducedMotion();
  const active = isActiveTask(task);
  const complete = task.phase === "completed";
  const failed = task.phase === "partial" || task.phase === "failed";
  const title = complete ? "Task completed" : failed ? "Action needs attention" : task.phase === "cancelled" ? "Task stopped" : task.message || phaseLabel(task.phase);
  const subtitle = failed
    ? task.message || phaseLabel(task.phase)
    : complete ? task.message || phaseLabel(task.phase)
      : task.progress ? `${task.progress.app_name} · Step ${task.progress.current_step} of ${task.progress.total_steps}`
        : phaseLabel(task.phase);
  const iconClass = complete ? "compact-icon success" : failed ? "compact-icon danger" : task.phase === "cancelled" ? "compact-icon muted-icon" : "compact-icon";

  return (
    <section className={`shell compact-shell with-icon${active ? " has-stop" : ""}${detailsExpanded ? " details-open" : ""}`} aria-label="Task status">
      <div className={iconClass} aria-hidden="true">
        {active ? <LoaderCircle className="loading-icon" size={17} />
          : complete ? <Check className="completed-icon" size={18} />
            : failed ? <TriangleAlert className="error-icon" size={18} />
              : <X size={16} />}
      </div>
      <button className="status-trigger grow" type="button" aria-expanded={detailsExpanded} onClick={onToggleDetails}>
        <span className="status-title">{title}</span>
        <span className="status-sub">{subtitle}</span>
      </button>
      {active && (
        <button className="text-control stop-control" type="button" disabled={task.cancellation_requested} onClick={onStop}>
          {task.cancellation_requested ? "Stopping" : "Stop"}
        </button>
      )}
      <button className="text-control detail-control" type="button" aria-expanded={detailsExpanded} onClick={onToggleDetails}>
        {detailsExpanded ? "Hide" : "Details"}
      </button>
      {task.progress && <ProgressTrack progress={task.progress} active={active && !detailsExpanded} />}
      <AnimatePresence initial={false}>
        {detailsExpanded && (
          <motion.div className="task-detail-motion" initial={reducedMotion ? { opacity: 0, height: 0 } : { opacity: 0, height: 0, y: 4 }} animate={{ opacity: 1, height: "auto", y: 0 }} exit={reducedMotion ? { opacity: 0, height: 0 } : { opacity: 0, height: 0, y: 2 }} transition={{ duration: reducedMotion ? 0.01 : 0.2, ease: [0.22, 1, 0.36, 1] }}>
            <div className="task-detail-list">
              {task.report
                ? <TaskReportDetails report={task.report} />
                : <><span className="mini-label">Task details</span><p>{task.message || "DeskAct is checking the request and supported target."}</p></>}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </section>
  );
}

function ProgressTrack({ progress, active }: { progress: TaskProgress; active: boolean }) {
  const [present, setPresent] = useState(true);
  const [expanded, setExpanded] = useState(false);
  const percent = Math.round((progress.current_step / Math.max(1, progress.total_steps)) * 100);

  useEffect(() => {
    setPresent(true);
    const reveal = window.requestAnimationFrame(() => setExpanded(true));
    if (active) return () => window.cancelAnimationFrame(reveal);
    const collapse = window.setTimeout(() => setExpanded(false), 700);
    const remove = window.setTimeout(() => setPresent(false), 900);
    return () => {
      window.cancelAnimationFrame(reveal);
      window.clearTimeout(collapse);
      window.clearTimeout(remove);
    };
  }, [active, progress.current_step, progress.total_steps]);

  if (!present) return null;
  return (
    <div className={`progress-shell${expanded ? " expanded" : ""}`}>
      <div className="progress-clip">
        <div className="progress-track" role="progressbar" aria-label="Task progress" aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent} aria-valuetext={`Step ${progress.current_step} of ${progress.total_steps}`}>
          <span className="progress-indicator" style={{ width: `${percent}%` }} />
        </div>
      </div>
    </div>
  );
}

function PreviewControls({ selected, onSelect }: { selected: PreviewState; onSelect: (state: PreviewState) => void }) {
  return (
    <aside className="preview-controls" aria-label="Prototype controls">
      <div className="preview-controls-heading">
        <strong>Window states</strong>
      </div>
      <div className="preview-state-grid">
        {previewStates.map(({ id, label }) => (
          <button key={id} className={selected === id ? "preview-state selected" : "preview-state"} type="button" aria-pressed={selected === id} onClick={() => onSelect(id)}>
            {label}
          </button>
        ))}
      </div>
    </aside>
  );
}

function TaskReportDetails({ report }: { report: TaskReport }) {
  return (
    <div className="task-detail-list">
      <strong>{report.status === "completed" ? "Verified results" : "Outcome details"}</strong>
      {report.evidence.map((item) => <p key={item.app_name}>{item.app_name}: open in a uniquely matched accessible window with a fresh snapshot.</p>)}
      {report.achieved_postconditions.map((item) => <p key={item}>{item}</p>)}
      {report.unresolved_steps.map((item) => <p className="warning-copy" key={item}>{item}</p>)}
      {!report.evidence.length && !report.unresolved_steps.length && <p>{report.summary}</p>}
    </div>
  );
}
