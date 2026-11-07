import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
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
    await user.click(controls.getByRole("button", { name: "Details" }));
    expect(screen.getByText("Preview only; no desktop action was dispatched or verified.")).toBeTruthy();
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

    await user.click(controls.getByRole("button", { name: "Approval" }));
    expect(screen.getByText("Move 3 selected files to the Recycle Bin?")).toBeTruthy();
    expect(screen.getByText("Preview only · no files will change")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Approve" }));
    expect(screen.getByText("Playback verified · Demo")).toBeTruthy();
    expect(screen.getByRole("progressbar", { name: "Task progress" })).toBeTruthy();
    await user.click(controls.getByRole("button", { name: "Error" }));
    expect(screen.getByRole("main", { name: "DeskAct Adaptive Island" }).className).toContain("state-error");
    expect(screen.getByText("Couldn't identify the right window")).toBeTruthy();
  });

  it("streams a voice preview live into the command field without a confirmation step", async () => {
    const user = userEvent.setup();
    render(<App />);
    const controls = within(screen.getByRole("complementary", { name: "Prototype controls" }));

    await user.click(controls.getByRole("button", { name: "Voice" }));
    expect(screen.getByText("Listening…")).toBeTruthy();
    expect(screen.getByText("Streaming transcript")).toBeTruthy();
    await waitFor(() => {
      expect((screen.getByRole("textbox", { name: "What would you like DeskAct to do?" }) as HTMLInputElement).value).toBe("Open Spotify and play my Liked Songs");
    }, { timeout: 3000 });
    expect(screen.queryByRole("button", { name: "Use text" })).toBeNull();

    await user.click(controls.getByRole("button", { name: "STT" }));
    expect(screen.getByText("Transcribing…")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Use text" })).toBeNull();
  });

  it("presents task status as a compact island with text controls", async () => {
    const user = userEvent.setup();
    const onToggleDetails = vi.fn();
    const onStop = vi.fn();
    render(<TaskStatus task={activeTask} detailsExpanded={false} onToggleDetails={onToggleDetails} onStop={onStop} />);

    expect(screen.getByRole("region", { name: "Task status" }).className).toContain("compact-shell");
    expect(screen.getByText("Opening Spotify")).toBeTruthy();
    expect(screen.getByRole("progressbar", { name: "Task progress" })).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Stop" }));
    expect(onStop).toHaveBeenCalledOnce();
    await user.click(screen.getByRole("button", { name: "Details" }));
    expect(onToggleDetails).toHaveBeenCalledOnce();
  });
});
