# Adaptive Island Windows prototype validation

This guide covers both the browser-only development preview and the current native Tauri application. The browser preview's Prototype controls switch among simulated UI states, including approval examples; they do not invoke native actions. The native application has no Prototype controls panel: its composer submits the currently supported catalog-grounded app-opening task, which can open a real desktop app without an approval step. Debug fixture mode replaces native execution with a labeled simulated result. Treat browser preview and fixture results as simulations, not native validation.

## Implemented for the prototype

- One Tauri v2 transparent, frameless, always-on-top window; no second passive/interactive window.
- Startup placement near the top center of the monitor reported by the native window, with a 16-logical-pixel top inset.
- The composer, passive-status preview, and attention-card preview request different native logical window sizes. These are geometry hypotheses.
- Basic web tests cover the preview controls and verify that the sample status/attention copy does not claim automation or approval.
- CI builds `deskact.exe` and `deskact-dev.exe` on Windows and uploads both as a short-lived artifact. This is not an installer.

## Not yet established

No interactive Windows desktop was available during implementation. Therefore non-activating status, foreground focus restoration, transparent-region click-through, monitor anchoring across a task, DPI/text scaling, multi-monitor fallback, and fullscreen behavior remain unverified. A transparent WebView background is not proof of native click-through. The native app can be reopened with its global shortcut or the tray's Open DeskAct item; tray Settings opens the separate settings window.

## Manual Windows validation checklist

Run the downloaded `deskact.exe` artifact in an interactive Windows session. The browser Prototype controls panel is absent from this native app. Native app-opening requests perform the supported desktop action; use fixture mode for a no-launch simulation and never treat its result as native verification.

- [ ] Confirm the window opens from the global shortcut and tray's Open DeskAct item, including after hiding it, on the current monitor near its top center and remains above other windows.
- [ ] Switch to another application while the passive-status preview is showing. Confirm it does not take foreground focus or receive unintended keystrokes.
- [ ] Open the composer and Settings from the native tray; confirm the native window bounds resize to fit each implemented view without clipping. Browser preview states and approval examples are not native controls.
- [ ] For a real supported app-opening check, submit an exact configured catalog app name and confirm the requested app opens. This is a real desktop action and does not show an approval prompt; use the fixture smoke test instead when no app should launch.
- [ ] Click on the transparent areas around the visible island and confirm the underlying app receives the click. Record if any part of the native bounds intercepts it.
- [ ] While another app has focus, interact deliberately with the composer; record focus acquisition and where focus returns when leaving the island.
- [ ] Run at 100%, 150%, and 200% display scaling; check that text and controls remain legible and no content is clipped.
- [ ] With two monitors of different sizes/scales, launch on each monitor and record placement. Disconnect the selected monitor and note whether the island remains reachable.
- [ ] Record Windows version/build, monitor layout, scaling, app build/commit, and pass/fail observations. Do not mark native behavior accepted from browser tests or CI compilation alone.

The findings decide whether one native window can safely support passive and interactive states or whether a carefully hidden native split is necessary. Any split must still present as one user-facing island.
