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

Open DeskAct from the tray or global shortcut, submit `open DeskAct Fixture App`, and confirm that the status changes from planning to a clearly marked simulated outcome. Verify that no application launches. To check Stop without native effects, set `$env:DESKACT_HUD_TEST_FIXTURE_DELAY_MS = '5000'`, submit the same request, then press Stop while the fixture is waiting; it should end as cancelled and explicitly report that no desktop action was dispatched. Clear both environment variables after the test. Do not distribute a debug build with fixture mode enabled.

## Active-task controls (fixture-only)

With fixture mode and `$env:DESKACT_HUD_TEST_FIXTURE_DELAY_MS = '10000'`, submit the fixture request and check these interactions before the delay ends:

1. Expand and collapse Details; the same request and revisioned status must remain intact.
2. Submit a second request while the first is active; DeskAct must show busy, not queue it. After the first task ends, the second request should not start by itself.
3. Press Escape to hide the island, then reopen with the global shortcut or tray; the same active request and status should reappear from Rust state.
4. Press Stop and confirm the final state is cancelled, no desktop action was dispatched, and another request can then be explicitly submitted.
5. Start another delayed fixture task and choose Quit from the tray or close control; confirm the uncertainty disclosure, choose Keep DeskAct open, then stop the fixture task.

To check the explicit quit path, start one more delayed fixture task, choose Quit anyway in the confirmation dialog, and confirm the app exits. No OS action is performed in fixture mode.

## Real supported path

For a real app-opening check, unset `DESKACT_HUD_TEST_FIXTURE`, install the documented `agent-desktop` prerequisite, open the island, and request one exact catalog app name. Confirm that completion appears only after Rust reports a unique accessible native window and fresh accessibility snapshot. Unsupported requests should fail without launching anything. This test runs only on a Windows desktop and may alter foreground/window state for the specifically requested app.

## What this smoke test does not prove

Browser component tests and fixture mode do not establish Windows focus restoration, non-activating passive status, transparent-region click-through, DPI behavior, monitor movement, or native verification. Those require the separate interactive Windows checklist. The fixture can be delayed for Stop testing. Cancellation cannot roll back an app opening already in flight; the task report communicates that uncertainty.
