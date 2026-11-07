import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";

type PrototypeState = "composer" | "working" | "attention" | "settings";
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

export default function App() {
  const [state, setState] = useState<PrototypeState>("composer");
  const [shortcut, setShortcut] = useState<ShortcutStatus>(defaultShortcut);
  const [shortcutInput, setShortcutInput] = useState(defaultShortcut.accelerator);
  const [saveError, setSaveError] = useState<string | null>(null);
  const requestInput = useRef<HTMLInputElement>(null);

  const showPrototypeStates = import.meta.env.DEV || import.meta.env.VITE_PROTOTYPE_PREVIEWS === "true";

  useLayoutEffect(() => {
    if (!isTauri()) return;

    const dimensions: Record<PrototypeState, [number, number]> = {
      composer: [600, 180],
      working: [460, 158],
      attention: [600, 238],
      settings: [520, 300],
    };
    const [width, height] = dimensions[state];
    void getCurrentWindow().setSize(new LogicalSize(width, height)).catch(() => {});
  }, [state]);

  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    const listeners: Array<() => void> = [];

    void listen("deskact://invoked", () => {
      setState("composer");
      requestAnimationFrame(() => requestInput.current?.focus());
    }).then((unlisten) => {
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
        <section className="surface composer-surface" aria-labelledby="composer-title">
          <label id="composer-title" className="sr-only" htmlFor="request">What would you like DeskAct to do?</label>
          <input ref={requestInput} id="request" placeholder="What would you like DeskAct to do?" />
          <button className="submit-button" type="button" aria-label="Submit request" title="Submit request" disabled>
            <span aria-hidden="true">↑</span>
          </button>
          <p className="hint">Prototype only · requests are not executed</p>
        </section>
      )}

      {state === "working" && (
        <section className="surface status-surface" aria-label="Simulated task status">
          <span className="activity-dot" aria-hidden="true" />
          <div className="status-copy">
            <strong>Simulated task in progress</strong>
            <span>No desktop action is running.</span>
          </div>
          <button className="quiet-button" type="button" aria-label="Expand simulated task details">Details</button>
        </section>
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
