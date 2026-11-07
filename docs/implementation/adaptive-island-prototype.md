# Adaptive Island Windows prototype validation

This is an early native-window prototype for Phase 2 issue #39. The app previews three explicitly simulated layouts and performs no desktop actions. It does not request approval. The preview buttons are test controls, not task controls.

## Implemented for the prototype

- One Tauri v2 transparent, frameless, always-on-top window; no second passive/interactive window.
- Startup placement near the top center of the monitor reported by the native window, with a 16-logical-pixel top inset.
- The composer, passive-status preview, and attention-card preview request different native logical window sizes. These are geometry hypotheses.
- Basic web tests cover the preview controls and verify that the sample status/attention copy does not claim automation or approval.
- CI builds `deskact.exe` and `deskact-dev.exe` on Windows and uploads both as a short-lived artifact. This is not an installer.

## Not yet established

No interactive Windows desktop was available during implementation. Therefore non-activating status, foreground focus restoration, transparent-region click-through, monitor anchoring across a task, DPI/text scaling, multi-monitor fallback, and fullscreen behavior remain unverified. A transparent WebView background is not proof of native click-through. Hide currently requires relaunch because the tray/reopen path is a later ticket.

## Manual Windows validation checklist

Run the downloaded `deskact.exe` artifact in an interactive Windows session. Do not interpret any preview as a real task.

- [ ] Confirm the window opens on the current monitor near its top center and remains above other windows.
- [ ] Switch to another application while the passive-status preview is showing. Confirm it does not take foreground focus or receive unintended keystrokes.
- [ ] Click through each preview control and confirm the *native* window bounds resize to fit the content without clipping.
- [ ] Click on the transparent areas around the visible island and confirm the underlying app receives the click. Record if any part of the native bounds intercepts it.
- [ ] While another app has focus, interact deliberately with the composer; record focus acquisition and where focus returns when leaving the island.
- [ ] Run at 100%, 150%, and 200% display scaling; check that text and controls remain legible and no content is clipped.
- [ ] With two monitors of different sizes/scales, launch on each monitor and record placement. Disconnect the selected monitor and note whether the island remains reachable.
- [ ] Record Windows version/build, monitor layout, scaling, app build/commit, and pass/fail observations. Do not mark native behavior accepted from browser tests or CI compilation alone.

The findings decide whether one native window can safely support passive and interactive states or whether a carefully hidden native split is necessary. Any split must still present as one user-facing island.
