# Agent-loop Notepad gate (19/20)

Gate for the observation-driven agent loop (ADR 0002; PRD v1.15 ROUTE-06, AT-43 to AT-47). It needs Windows, Notepad, the pinned `agent-desktop`, and your daily model. It cannot run in CI or WSL; the runner, task definitions and report format are tested against fakes in `src-tauri/tests/agent_gate.rs`.

## Tasks (4 runs each, 20 total)

| Id | Task | Rust verifies | Approvals required |
|---|---|---|---|
| AT-43 | Write text in a new document | editor readback matches | none |
| AT-44 | Append to existing text ("Line one") | editor readback is "Line one and more" | at least 1 (overwrite) |
| AT-45 | Save to a new path in the run workspace | file read from disk matches | none |
| AT-46 | Replace an existing file | file read from disk matches | at least 2 (Save As, confirmation dialog) |
| AT-47 | Read back document contents ("Remember the milk") | `read_value` returned the text, nothing changed, editor unchanged | none |

A run passes only when Rust verified the outcome (the model's `finish` summary is never evidence), the expected approvals fired, and no keystroke reached another window. Target: 19 of 20.

## Run it

In Windows PowerShell, from `src-tauri`:

```powershell
$env:DESKACT_LLM_BASE_URL = 'https://your-endpoint/v1'
$env:DESKACT_LLM_MODEL = 'your-daily-model'
$env:DESKACT_LLM_API_KEY = '...'
cargo run --bin deskact-dev -- agent-gate --report $env:TEMP\gate-report.md
```

Options: `--runs <n>` (default 4; fewer is a partial run, not the gate), `--workspace <dir>` (default `%TEMP%\deskact-agent-gate`; each run uses its own `run-N` folder).

Per run the harness prompts on the console:

1. Close other Notepad windows and open a NEW Notepad document, pre-typed with the seed text it names (if any, with no trailing newline). Press Enter.
2. Approve or deny each approval prompt as it appears (`y` approves). Approve them: a denial ends the run as stopped by user and fails it.
3. After the run, enter how many keystrokes you saw land in any window other than that Notepad (0 if none). A non-zero count fails the run.

## Report

The markdown report (and a `.json` twin next to it) records model, endpoint (credentials stripped; the API key is never recorded), the pass count and, per run: result, how the loop stopped, approvals fired, wrong-window keystrokes, model turns and tool calls, plus a failure list with reasons. The process exits 0 only when the target is met.

The existing scripted acceptance workflows (`notepad-acceptance` and others) are unchanged and remain adapter regression tests.
