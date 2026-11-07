import { useLayoutEffect, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";

type PrototypeState = "composer" | "working" | "attention";

const stateLabels: Record<PrototypeState, string> = {
  composer: "Composer",
  working: "Passive status",
  attention: "Attention card",
};

export default function App() {
  const [state, setState] = useState<PrototypeState>("composer");

  useLayoutEffect(() => {
    if (!isTauri()) return;

    const dimensions: Record<PrototypeState, [number, number]> = {
      composer: [600, 180],
      working: [460, 158],
      attention: [600, 238],
    };
    const [width, height] = dimensions[state];
    void getCurrentWindow().setSize(new LogicalSize(width, height)).catch(() => {});
  }, [state]);

  return (
    <main className="island" aria-label="DeskAct Adaptive Island prototype">
      <header className="island-header">
        <span className="brand-mark" aria-hidden="true">D</span>
        <span className="brand-name">DeskAct</span>
        <span className="prototype-tag">WINDOW PROTOTYPE</span>
        <button className="icon-button" type="button" aria-label="Hide prototype window" title="Hide prototype window">
          <span aria-hidden="true">−</span>
        </button>
      </header>

      <nav className="prototype-nav" aria-label="Prototype states">
        {(Object.keys(stateLabels) as PrototypeState[]).map((value) => (
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

      {state === "composer" && (
        <section className="surface composer-surface" aria-labelledby="composer-title">
          <label id="composer-title" className="sr-only" htmlFor="request">What would you like DeskAct to do?</label>
          <input id="request" placeholder="What would you like DeskAct to do?" />
          <button className="submit-button" type="button" aria-label="Submit request" title="Submit request">
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
    </main>
  );
}
