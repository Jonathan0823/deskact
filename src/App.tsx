import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type FormEvent } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Check, CircleX, LoaderCircle, Mic, Send, TriangleAlert, X } from "lucide-react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";

type PrototypeState = "composer" | "working" | "attention" | "settings" | "voice";
type PreviewState = "hidden" | "command" | "voice" | "working" | "settings" | "approval" | "destructive" | "removed" | "destroyed" | "done" | "error" | "cancelled";
const previewStates: Array<{ id: PreviewState; label: string }> = [
  { id: "hidden", label: "Hidden" },
  { id: "command", label: "Command" },
  { id: "voice", label: "Voice" },
  { id: "working", label: "Working" },
  { id: "settings", label: "Settings" },
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
type ThemeMode = "light" | "dark" | "system";
type AppearanceSettings = { theme: ThemeMode; opacity: number; micaEnabled: boolean };
type ShortcutStatus = {
  accelerator: string;
  registered: boolean;
  error: string | null;
};

const appearanceStorageKey = "deskact-appearance-v1";
const islandTransitionMs = 180;
const defaultAppearance: AppearanceSettings = { theme: "light", opacity: 48, micaEnabled: true };
function readAppearance(): AppearanceSettings {
  try {
    const saved = JSON.parse(localStorage.getItem(appearanceStorageKey) ?? "null") as (Partial<AppearanceSettings> & { blur?: number }) | null;
    if (!saved) return defaultAppearance;
    return {
      theme: saved.theme === "dark" || saved.theme === "system" ? saved.theme : "light",
      opacity: typeof saved.opacity === "number" ? Math.max(18, Math.min(85, saved.opacity)) : defaultAppearance.opacity,
      micaEnabled: typeof saved.micaEnabled === "boolean" ? saved.micaEnabled : typeof saved.blur === "number" ? saved.blur > 0 : defaultAppearance.micaEnabled,
    };
  } catch {
    return defaultAppearance;
  }
}

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
    case "settings": return "settings";
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
  const phase: TaskPhase | null = state === "working" ? "running"
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
  const [appearance, setAppearance] = useState<AppearanceSettings>(readAppearance);
  const [systemPrefersDark, setSystemPrefersDark] = useState(false);
  const [shortcut, setShortcut] = useState<ShortcutStatus>(defaultShortcut);
  const [shortcutInput, setShortcutInput] = useState(defaultShortcut.accelerator);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [request, setRequest] = useState("");
  const [task, setTask] = useState<TaskSnapshot | null>(null);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [quitRequested, setQuitRequested] = useState(false);
  const requestInput = useRef<HTMLInputElement>(null);
  const islandRef = useRef<HTMLElement>(null);
  const taskRef = useRef<TaskSnapshot | null>(null);
  const hidePending = useRef(false);
  const browserPreview = import.meta.env.DEV && !isTauri();
  const prefersReducedMotion = useReducedMotion();
  const resolvedTheme = appearance.theme === "system" ? (systemPrefersDark ? "dark" : "light") : appearance.theme;
  const shellStyle = { "--glass-opacity": appearance.opacity / 100 } as CSSProperties;
  const displayedState = browserPreview ? previewView(previewState) : islandVisible ? state : "hidden";
  const displayedTask = browserPreview ? previewTask(previewState, previewStep) : task;
  const lastPreviewState = useRef<PreviewState>("command");
  if (browserPreview && previewState !== "hidden") lastPreviewState.current = previewState;
  const contentState = browserPreview && previewState === "hidden"
    ? previewView(lastPreviewState.current)
    : displayedState === "hidden" ? state : displayedState;
  const contentTask = browserPreview && previewState === "hidden"
    ? previewTask(lastPreviewState.current, previewStep)
    : displayedTask;
  const destructiveApproval = browserPreview && (previewState === "destructive" || (previewState === "hidden" && lastPreviewState.current === "destructive"));

  function selectPreviewState(next: PreviewState) {
    if (next === "working") setPreviewStep(0);
    if (next === "voice") setStreamedTranscript("");
    setPreviewState(next);
  }

  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const update = () => setSystemPrefersDark(media.matches);
    update();
    media.addEventListener?.("change", update);
    return () => media.removeEventListener?.("change", update);
  }, []);

  useEffect(() => {
    try { localStorage.setItem(appearanceStorageKey, JSON.stringify(appearance)); } catch { /* Appearance remains available for this session. */ }
  }, [appearance]);

  useEffect(() => {
    if (!isTauri()) return;
    void invoke("set_backdrop_material", { enabled: appearance.micaEnabled }).catch(() => {});
  }, [appearance.micaEnabled]);

  function setIslandVisibility(visible: boolean) {
    if (isTauri()) {
      void invoke("set_backdrop_visibility", {
        visible,
        durationMs: prefersReducedMotion ? 0 : islandTransitionMs,
      }).catch(() => {});
    }
    setIslandVisible(visible);
  }

  function dismissIsland() {
    if (browserPreview) {
      setPreviewState("hidden");
      return;
    }
    if (displayedState === "hidden") {
      void invoke("dismiss_island").catch(() => {});
      return;
    }
    hidePending.current = true;
    setIslandVisibility(false);
  }

  function morphWindowToContent() {
    if (!isTauri()) return;

    let width: number;
    let cornerRadius: number;
    if (quitRequested || state === "attention") {
      width = 500;
      cornerRadius = 26;
    } else if (state === "composer") {
      width = 460;
      cornerRadius = 52;
    } else if (state === "settings") {
      width = 480;
      cornerRadius = 24;
    } else if (state === "voice") {
      width = 396;
      cornerRadius = 52;
    } else {
      width = task?.phase === "completed" || task?.phase === "cancelled" ? 340 : 430;
      cornerRadius = 44;
    }
    const height = Math.ceil(islandRef.current?.scrollHeight ?? 88);
    const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    void invoke("morph_island_window", { width, height, cornerRadius, reducedMotion }).catch(() => {});
  }

  useLayoutEffect(() => {
    morphWindowToContent();
  }, [state, quitRequested, task?.phase]);

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
      hidePending.current = false;
      setIslandVisibility(true);
      setState("settings");
      void refreshShortcut();
    }).then((unlisten) => {
      if (disposed) unlisten();
      else listeners.push(unlisten);
    });
    void listen("deskact://quit-requested", () => {
      hidePending.current = false;
      setIslandVisibility(true);
      setQuitRequested(true);
    }).then((unlisten) => {
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
    hidePending.current = false;
    setIslandVisibility(true);
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
    <div className={`app-shell ${browserPreview ? "web-preview" : "tauri-root"} theme-${resolvedTheme}`} style={shellStyle}>
      <motion.main
        ref={islandRef}
        className={`island state-${visualState}${quitRequested ? " quit-confirming" : ""}${displayedState === "hidden" ? " is-hidden" : ""}`}
        aria-label="DeskAct Adaptive Island"
        aria-hidden={displayedState === "hidden"}
        inert={displayedState === "hidden"}
        initial={false}
        animate={displayedState === "hidden" ? "hidden" : "visible"}
        variants={{ visible: { opacity: 1, y: 0, scale: 1 }, hidden: { opacity: 0, y: -4, scale: 0.97 } }}
        transition={{ duration: prefersReducedMotion ? 0 : islandTransitionMs / 1000, ease: [0.22, 1, 0.36, 1] }}
        onAnimationComplete={(definition) => {
          if (definition === "hidden" && hidePending.current && !browserPreview) {
            hidePending.current = false;
            void invoke("dismiss_island").catch(() => {});
          }
        }}
      >
      <AnimatePresence mode="popLayout" initial={false} onExitComplete={morphWindowToContent}>
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
                <button className="send-button" type="submit" aria-label="Run command" disabled={!request.trim()}>
                  <Send size={24} aria-hidden="true" />
                </button>
              </div>
            </form>
            {submitError && <p className="error-message" role="alert">{submitError}</p>}
          </section>
        </>
      )}

      {contentState === "voice" && (
        <section className="shell compact-shell voice-shell" aria-label="Voice input">
          <div className="voice-control-row">
            <div className="compact-icon mic-active"><Mic size={24} aria-hidden="true" /></div>
            <div className="voice-wave-slot"><div className="wave" aria-hidden="true">{Array.from({ length: 17 }, (_, index) => <i key={index} />)}</div></div>
            <button className="glass-icon" type="button" aria-label="Cancel voice input" onClick={() => selectPreviewState("command")}><X size={22} aria-hidden="true" /></button>
          </div>
          <div className="voice-transcript-row">
            <StreamingText text={streamedTranscript} />
            <span className="sr-only" role="status">Listening for voice input.</span>
          </div>
        </section>
      )}

      {contentState === "working" && (
        contentTask ? (
          <TaskStatus
            task={contentTask}
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
            <TriangleAlert className={`approval-icon error-icon ${destructiveApproval ? "danger" : "warn"}`} size={24} aria-hidden="true" />
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

      {contentState === "settings" && (
        <section className="surface settings-surface" aria-labelledby="settings-title">
          <h1 id="settings-title">Appearance</h1>
          <div className="theme-tabs" role="group" aria-label="Color theme">
            {(["light", "dark", "system"] as const).map((theme) => <button key={theme} type="button" aria-pressed={appearance.theme === theme} onClick={() => setAppearance((value) => ({ ...value, theme }))}>{theme[0].toUpperCase() + theme.slice(1)}</button>)}
          </div>
          <div className="range-setting"><label htmlFor="glass-opacity">Glass opacity</label><output>{appearance.opacity}%</output><input id="glass-opacity" type="range" min="18" max="85" value={appearance.opacity} onChange={(event) => setAppearance((value) => ({ ...value, opacity: Number(event.target.value) }))} /></div>
          <div className="toggle-setting">
            <span id="mica-material-label">Desktop glass blur</span>
            <button id="mica-material" className="toggle-switch" type="button" role="switch" aria-labelledby="mica-material-label" aria-checked={appearance.micaEnabled} onClick={() => setAppearance((value) => ({ ...value, micaEnabled: !value.micaEnabled }))}>
              <span className="toggle-thumb" />
            </button>
            <span className="toggle-value">{appearance.micaEnabled ? "On" : "Off"}</span>
          </div>
          {!browserPreview && <>
          <h2 className="settings-subheading">Global shortcut</h2>
          <p className="settings-copy">Open DeskAct from another application.</p>
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
          </>}
          <div className="button-row">
            <button className="quiet-button" type="button" onClick={() => browserPreview ? selectPreviewState("command") : setState("composer")}>Back</button>
            {!browserPreview && <button className="primary-button" type="button" onClick={() => void saveShortcut()}>Save shortcut</button>}
          </div>
        </section>
      )}
      </motion.div>
      </AnimatePresence>
      </motion.main>
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

export function TaskStatus({ task, onStop }: { task: TaskSnapshot; onStop: () => void }) {
  const active = isActiveTask(task);
  const complete = task.phase === "completed";
  const cancelled = task.phase === "cancelled";
  const failed = task.phase === "partial" || task.phase === "failed";
  const label = complete ? task.message || "Task completed"
    : failed ? task.message || "Action needs attention"
      : task.phase === "cancelled" ? "Task cancelled"
        : task.message || phaseLabel(task.phase);

  return (
    <section className="shell task-shell" aria-label="Task status">
      <span className="task-label">{label}</span>
      {active ? <span className="loader" aria-label="Working" />
        : complete ? <Check className="done-icon" size={28} aria-label="Completed" />
          : failed ? <TriangleAlert className="error-icon" size={26} aria-label="Needs attention" />
            : cancelled ? <CircleX className="cancelled-icon" size={28} aria-label="Cancelled" />
              : null}
      {active && <button className="glass-icon" type="button" aria-label={task.cancellation_requested ? "Stopping task" : "Cancel task"} disabled={task.cancellation_requested} onClick={onStop}><X size={22} aria-hidden="true" /></button>}
    </section>
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
