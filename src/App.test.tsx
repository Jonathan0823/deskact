import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";

afterEach(cleanup);
import App from "./App";

describe("Adaptive Island window prototype", () => {
  it("starts with a non-executing composer preview", () => {
    render(<App />);

    expect(screen.getByRole("textbox", { name: "What would you like DeskAct to do?" })).toBeTruthy();
    expect(screen.getByText("Prototype only · requests are not executed in browser preview")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Submit request" }).hasAttribute("disabled")).toBe(true);
  });

  it("does not dispatch browser-preview submissions", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.type(screen.getByRole("textbox", { name: "What would you like DeskAct to do?" }), "open DeskAct Fixture App");
    await user.click(screen.getByRole("button", { name: "Submit request" }));

    expect(screen.getByText("Prototype only · requests are not executed in browser preview")).toBeTruthy();
  });

  it("shows simulated status and attention previews through explicit controls", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.click(screen.getByRole("button", { name: "Passive status" }));
    expect(screen.getByRole("region", { name: "Simulated task status" })).toBeTruthy();
    expect(screen.getByText("No desktop action is running.")).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Attention card" }));
    expect(screen.getByRole("heading", { name: "Window behavior needs validation" })).toBeTruthy();
    expect(screen.getByText("This prototype does not request approval or control another app.")).toBeTruthy();
  });

  it("returns from attention preview to the composer", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.click(screen.getByRole("button", { name: "Attention card" }));
    await user.click(screen.getByRole("button", { name: "Return to composer" }));

    expect(screen.getByRole("textbox", { name: "What would you like DeskAct to do?" })).toBeTruthy();
  });
});
