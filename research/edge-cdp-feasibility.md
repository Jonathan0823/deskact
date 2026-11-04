# Microsoft Edge CDP feasibility and session constraints

Context: [Wayfinder research ticket #5](https://github.com/Jonathan0823/deskact/issues/5).

## Conclusion

Documented capability supports attaching `agent-browser` to an already-running Edge instance via its local CDP port: Edge exposes a DevTools Protocol matching Chrome CDP, while agent-browser documents CDP attachment by numeric port and compatibility with CDP browsers. The docs do **not** establish that a particular Windows installation, enterprise policy, Edge build, or agent-browser build will work; run the target-machine check below before promising support. [Microsoft Edge CDP](https://learn.microsoft.com/en-us/microsoft-edge/devtools/protocol/) · [agent-browser CDP mode](https://agent-browser.dev/cdp-mode)

## Findings

1. **Dedicated, safer setup.** Microsoft documents closing all Edge instances, then launching `msedge.exe --remote-debugging-port=9222`; a distinct profile can optionally be selected with `--user-data-dir=<dedicated path>`. Use a dedicated profile directory and a free dedicated port, not the everyday profile. Edge documents the local discovery endpoint as `http://localhost:9222/json/list`. [Microsoft Edge CDP](https://learn.microsoft.com/en-us/microsoft-edge/devtools/protocol/)

   Example (Windows command prompt; substitute actual Edge path):
   ```bat
   "C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe" --remote-debugging-port=9222 --user-data-dir="%LOCALAPPDATA%\DeskAct\Edge-CDP-Profile"
   ```
   This is a proposed invocation assembled from documented switches, not a validated target-machine command. Microsoft documentation specifies `localhost` endpoints, but does not document an explicit bind-address switch or guarantee loopback-only binding. Verify the listener is loopback-only on the target; do not assume it.

2. **Attach, do not launch via agent-browser.** `agent-browser connect 9222` attaches once and subsequent commands use that connection; alternatively `agent-browser --cdp 9222 ...` attaches per command. The documentation describes CDP as connecting to an existing browser, not launching Edge for this workflow. `--profile` is not a way to select an Edge profile during CDP attach: agent-browser’s profile-copy support lists Chrome, Chrome Canary, Chromium, and Brave, not Edge. [CDP mode](https://agent-browser.dev/cdp-mode) · [Sessions/profile docs](https://agent-browser.dev/sessions)

3. **Security and existing profile risk.** agent-browser warns that local processes can control the browser through the debugging port and says to use trusted machines and close Chrome when done. A CDP endpoint grants powerful browser control; do not expose it to a network or attach to a user’s ordinary browser/profile. Edge’s documentation instructs closing all Edge instances before starting with the port and offers a separate user-data directory. Browser/profile reuse risks exposing logged-in sessions and personal data; use only an explicitly approved, dedicated profile and account. [Sessions/auth docs](https://agent-browser.dev/sessions) · [Microsoft Edge CDP](https://learn.microsoft.com/en-us/microsoft-edge/devtools/protocol/)

4. **Tab selection and shared-session limitations.** CDP lists attachable page targets; agent-browser can attach to the browser and its documented session/tab features can bind sessions to target tabs. Without `--pin-tab`, shared-browser sessions can operate on the shared active tab; pinning makes tab binding strict and prevents silently switching after a bound tab disappears. This is tab selection, **not** isolated browser state. Any attached user browser shares its cookies, authentication, storage, and live tabs with automation. [Microsoft Edge CDP](https://learn.microsoft.com/en-us/microsoft-edge/devtools/protocol/) · [CDP tab pinning](https://agent-browser.dev/cdp-mode) · [Sessions](https://agent-browser.dev/sessions)

5. **Auth ownership and persistence.** An attached Edge profile owns the live authentication state; do not imply agent-browser `--session` creates an isolated browser when using shared CDP. agent-browser documents session isolation for separately launched instances, while CDP sessions sharing one browser are separated only by tab selection. Persistent agent-browser restore/state facilities can store cookies/tokens; state files may contain plaintext session tokens. Avoid saving/importing auth unless explicitly required, protect any state file, and never commit it. [Sessions](https://agent-browser.dev/sessions)

6. **Policy gate.** Edge’s `RemoteDebuggingAllowed` policy, supported on Windows Edge 93+, controls whether remote debugging is permitted; enabled or unconfigured allows the command-line port/pipe switches, disabled prohibits remote debugging. A managed device may therefore block the workflow. [RemoteDebuggingAllowed policy](https://learn.microsoft.com/en-us/deployedge/microsoft-edge-policies/remotedebuggingallowed)

## Exact target-machine check still required

On the supported Windows setup, record Edge version and agent-browser version; confirm policy permits remote debugging; close Edge; launch the exact Edge binary with a new dedicated user-data directory and chosen port; inspect the listening address to confirm it is loopback-only; request `http://localhost:<port>/json/version` and `/json/list`; then run `agent-browser --cdp <port> snapshot` and verify it attaches to the intended page. Also test `connect <port>`, tab listing/pinning, and close/restart behavior. Confirm that no ordinary Edge profile/session is touched and that terminating the dedicated instance removes the listener. Until this empirical check passes, state feasibility as documented CDP compatibility only—not verified support on the target Windows setup.

## Sources

- [agent-browser CDP Mode](https://agent-browser.dev/cdp-mode) — attachment, commands, shared tabs and pinning.
- [agent-browser Sessions](https://agent-browser.dev/sessions) — session isolation, profile support, authentication and state safety.
- [Microsoft Edge DevTools Protocol](https://learn.microsoft.com/en-us/microsoft-edge/devtools/protocol/) — Edge launch switches, profile option, localhost endpoints and target discovery.
- [Microsoft Edge RemoteDebuggingAllowed policy](https://learn.microsoft.com/en-us/deployedge/microsoft-edge-policies/remotedebuggingallowed) — policy gate.
