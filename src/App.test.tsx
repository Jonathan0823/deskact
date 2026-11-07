import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import App, { TaskStatus } from "./App";

afterEach(cleanup);

const activeTask = {
  task_id: 7,
  revision: 1,
  request: "Open Spotify",
  phase: "running" as const,
  message: "Opening Spotify",
  progress: { app_name: "Spotify", current_step: 1, total_steps: 3 },
  report: null,
  test_fixture: false,
  cancellation_requested: false,
};

describe("Adaptive Island", () => {
  it("renders the title-free command capsule and dev-only preview controls", () => {
    render(<App />);

    expect(screen.getByRole("textbox", { name: "What would you like DeskAct to do?" })).toBeTruthy();
    expect(screen.getByRole("main", { name: "DeskAct Adaptive Island" }).className).toContain("state-composer");
    expect(screen.getByRole("main", { name: "DeskAct Adaptive Island" }).querySelector(".brand-spark")).toBeNull();
    expect(screen.queryByText("DeskAct", { selector: ".title" })).toBeNull();
    expect(screen.getByRole("button", { name: "Run command" }).hasAttribute("disabled")).toBe(true);
    expect(screen.getByRole("complementary", { name: "Prototype controls" })).toBeTruthy();
  });

  it("does not dispatch browser-preview submissions", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.type(screen.getByRole("textbox", { name: "What would you like DeskAct to do?" }), "open DeskAct Fixture App");
    await user.click(screen.getByRole("button", { name: "Run command" }));

    expect(screen.getByRole("main", { name: "DeskAct Adaptive Island" }).className).toContain("state-composer");
    expect(screen.queryByRole("region", { name: "Task status" })).toBeNull();
  });

  it("switches dev-web presets without dispatching real actions", async () => {
    const user = userEvent.setup();
    render(<App />);
    const controls = within(screen.getByRole("complementary", { name: "Prototype controls" }));

    await user.click(controls.getByRole("button", { name: "Working" }));
    expect(screen.getByText("Opening Spotify")).toBeTruthy();
    await user.click(controls.getByRole("button", { name: "Settings" }));
    expect(screen.getByRole("group", { name: "Color theme" })).toBeTruthy();
    const darkMode = screen.getByRole("button", { name: "Dark" });
    await user.click(darkMode);
    expect(document.querySelector(".app-shell")?.classList.contains("theme-dark")).toBe(true);
    const opacity = screen.getByLabelText("Glass opacity");
    const backdropBlur = screen.getByRole("switch", { name: "Mica material" });
    expect(backdropBlur.getAttribute("aria-checked")).toBe("true");
    await user.click(backdropBlur);
    expect(backdropBlur.getAttribute("aria-checked")).toBe("false");
    expect(document.querySelector(".app-shell")?.getAttribute("style")).toContain("--glass-blur: 0px");
    await user.click(backdropBlur);
    expect(backdropBlur.getAttribute("aria-checked")).toBe("true");
    fireEvent.change(opacity, { target: { value: "62" } });
    expect((opacity as HTMLInputElement).value).toBe("62");
    expect(document.querySelector(".app-shell")?.getAttribute("style")).toContain("--glass-opacity: 0.62");
    await user.click(screen.getByRole("button", { name: "Back" }));
    await user.click(controls.getByRole("button", { name: "Hidden" }));
    expect(screen.queryByRole("main", { name: "DeskAct Adaptive Island" })).toBeNull();
    expect(controls.getByRole("button", { name: "Hidden" })).toBeTruthy();
  });

  it("hides and restores the island without removing the browser preview controls", async () => {
    const user = userEvent.setup();
    render(<App />);
    const controls = within(screen.getByRole("complementary", { name: "Prototype controls" }));

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("main", { name: "DeskAct Adaptive Island" })).toBeNull();
    expect(document.querySelector("main")?.classList.contains("is-hidden")).toBe(true);
    await user.click(controls.getByRole("button", { name: "Command" }));
    expect(screen.getByRole("main", { name: "DeskAct Adaptive Island" })).toBeTruthy();
    expect(document.querySelector("main")?.classList.contains("is-hidden")).toBe(false);
  });

  it("previews approval and completion states without changing desktop state", async () => {
    const user = userEvent.setup();
    render(<App />);
    const controls = within(screen.getByRole("complementary", { name: "Prototype controls" }));

    await user.click(controls.getByRole("button", { name: "Remove" }));
    expect(document.querySelector(".approval-text")?.textContent?.replace(/\s+/g, " ").trim()).toBe("Move 3 selected files to the Recycle Bin?");
    expect(screen.getByText("Can be restored from the Recycle Bin")).toBeTruthy();
    expect(document.querySelector(".approval-shell")?.classList.contains("approval-reversible")).toBe(true);
    expect(document.querySelector(".approval-icon")?.classList.contains("compact-icon")).toBe(false);
    expect(document.querySelector(".approval-icon")?.classList.contains("error-icon")).toBe(true);
    expect(document.querySelector(".approval-shell .reversible-action")).toBeTruthy();
    expect(within(screen.getByRole("main", { name: "DeskAct Adaptive Island" })).queryByText(/demo|fixture|preview only/i)).toBeNull();
    await user.click(screen.getByRole("button", { name: "Move to Recycle Bin" }));
    expect(screen.getByText("Moved to Recycle Bin")).toBeTruthy();
    expect(screen.queryByRole("progressbar", { name: "Task progress" })).toBeNull();
    await user.click(controls.getByRole("button", { name: "Error" }));
    expect(screen.getByRole("main", { name: "DeskAct Adaptive Island" }).className).toContain("state-error");
    expect(screen.getByText("Couldn't identify the right window")).toBeTruthy();
  });

  it("shows a separately red-coded permanent-deletion approval", async () => {
    const user = userEvent.setup();
    render(<App />);
    const controls = within(screen.getByRole("complementary", { name: "Prototype controls" }));

    await user.click(controls.getByRole("button", { name: "Destruction" }));
    expect(document.querySelector(".approval-text")?.textContent?.replace(/\s+/g, " ").trim()).toBe("Permanently delete 3 selected files?");
    expect(screen.getByText("Cannot be undone")).toBeTruthy();
    expect(document.querySelector(".approval-shell")?.classList.contains("approval-destructive")).toBe(true);
    expect(document.querySelector(".approval-icon")?.classList.contains("error-icon")).toBe(true);
    expect(document.querySelector(".approval-shell .destructive-action")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Delete permanently" }));
    expect(screen.getByText("Permanently deleted")).toBeTruthy();
  });

  it("auto-fills voice transcription without a confirmation step", async () => {
    const user = userEvent.setup();
    render(<App />);
    const controls = within(screen.getByRole("complementary", { name: "Prototype controls" }));

    await user.click(controls.getByRole("button", { name: "Voice" }));
    expect(screen.queryByText("Listening…")).toBeNull();
    expect(screen.getByRole("region", { name: "Voice input" })).toBeTruthy();
    const voiceControls = document.querySelector(".voice-control-row");
    expect(voiceControls?.querySelector(".mic-active")).toBeTruthy();
    expect(voiceControls?.querySelector(".voice-wave-slot .wave")).toBeTruthy();
    expect(voiceControls?.querySelector('[aria-label="Cancel voice input"]')).toBeTruthy();
    expect(document.querySelector(".voice-transcript-row .transcript-line")).toBeTruthy();
    expect(screen.queryByText("Streaming transcript")).toBeNull();
    await waitFor(() => expect(document.querySelector(".transcript-line")?.textContent?.length).toBeGreaterThan(0));
    await waitFor(() => {
      expect((screen.getByRole("textbox", { name: "What would you like DeskAct to do?" }) as HTMLInputElement).value).toBe("Open Spotify and play my Liked Songs");
    }, { timeout: 3000 });
    expect(screen.queryByRole("button", { name: "Use text" })).toBeNull();

    expect(controls.queryByRole("button", { name: "STT" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Use text" })).toBeNull();
  });

  it("shows an animated neutral icon for cancelled tasks", () => {
    const cancelledTask = { ...activeTask, phase: "cancelled" as const, message: "Task cancelled" };
    render(<TaskStatus task={cancelledTask} onStop={() => {}} />);

    expect(screen.getByText("Task cancelled")).toBeTruthy();
    expect(screen.getByLabelText("Cancelled")).toBeTruthy();
    expect(document.querySelector(".task-shell .cancelled-icon")).toBeTruthy();
    expect(document.querySelector(".task-shell .loader")).toBeNull();
  });

  it("presents task status as a simple glass pill with cancellation", async () => {
    const user = userEvent.setup();
    const onStop = vi.fn();
    render(<TaskStatus task={activeTask} onStop={onStop} />);

    expect(screen.getByRole("region", { name: "Task status" }).className).toContain("task-shell");
    expect(screen.getByText("Opening Spotify")).toBeTruthy();
    expect(screen.queryByRole("progressbar", { name: "Task progress" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Cancel task" }));
    expect(onStop).toHaveBeenCalledOnce();
  });
});
