# Adaptive Island task smoke test

The Tauri app currently routes one explicit `open <catalog app>` request through the shared Rust task planner/executor and displays authoritative task snapshots. Hiding the window does not own or cancel the Rust task. This is not a released general agent: only the existing verified app-opening capability is enabled.

## Non-destructive fixture mode

Fixture mode is compiled only in debug builds. It uses the real local request validation and catalog-grounded routing, then supplies a deterministic fake accessible-window result in place of native execution. No app is launched and the UI labels the outcome as fixture data rather than native verification.

In Windows PowerShell, create a temporary catalog entry and start the development app:

```powershell
$catalog = Join-Path $env:TEMP 'deskact-hud-fixture.json'
@'
{
  "version": 1,
  "entries": [
    {
      "id": "hud-fixture-app",
      "name": "DeskAct Fixture App",
      "target": "C:\\Windows\\System32\\notepad.exe",
      "arguments": "",
      "working_directory": "",
      "source": "user-configured",
      "aliases": []
    }
  ]
}
'@ | Set-Content -Encoding ascii $catalog
$env:DESKACT_APP_CATALOG = $catalog
$env:DESKACT_HUD_TEST_FIXTURE = '1'
npm ci
npm run dev:app
```

Open DeskAct from the tray or global shortcut, submit `open DeskAct Fixture App`, and confirm that the status changes from planning to a clearly marked simulated outcome. Verify that no application launches. Clear the environment variables after the test. Do not distribute a debug build with fixture mode enabled.

## Real supported path

For a real app-opening check, unset `DESKACT_HUD_TEST_FIXTURE`, install the documented `agent-desktop` prerequisite, open the island, and request one exact catalog app name. Confirm that completion appears only after Rust reports a unique accessible native window and fresh accessibility snapshot. Unsupported requests should fail without launching anything. This test runs only on a Windows desktop and may alter foreground/window state for the specifically requested app.

## What this smoke test does not prove

Browser component tests and fixture mode do not establish Windows focus restoration, non-activating passive status, transparent-region click-through, DPI behavior, monitor movement, or native verification. Those require the separate interactive Windows checklist. Stop/cancel and fully recoverable attention states are implemented in later tickets.
