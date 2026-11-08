# DeskAct

DeskAct is a Windows-first, accessibility-first desktop assistant for finding, opening, and controlling supported apps. The product is in the specification and Windows validation stage; the DeskAct application is not yet released. The Adaptive Island Windows host is an early prototype: native focus, click-through, DPI, and monitor behavior still require live Windows validation. See the [Product Requirements Document](docs/DeskAct_PRD_v1.14.md), [Glossary](GLOSSARY.md), and [Adaptive Island UI/UX spec](design/ui/DeskAct_Adaptive_Island_UI_Spec_v0.1.md).

## Adaptive Island app prototype

DeskAct is one Tauri v2 application, not separate UI and AI apps. `deskact.exe` is the Rust/Tauri host; the React/Tailwind UI runs in its WebView and communicates with Rust through typed Tauri commands and events. The shared Rust core owns routing, task execution, verification, and the LLM client; endpoint credentials and model requests stay in Rust. The configured LLM endpoint is external to DeskAct. The separate `deskact-dev.exe` is a developer-only CLI test harness, not another desktop-app version; CI publishes it as a separate developer artifact, not alongside the app executable.

The app's composer submits the currently supported app-opening workflow; Rust owns task state and reports verified native results. Active tasks expose Details and Stop controls, and quitting during work requires explicit confirmation that in-flight effects may be uncertain and are not rolled back. Debug fixture mode exercises this path without launching apps. The UI uses the Soft Glass translucent surface with light, dark, and system themes, adjustable surface opacity, and a native Windows Desktop Acrylic backdrop toggle in Settings. It uses DWM's system-backdrop API on Windows 11 22H2 and later, requires no separately installed Windows App SDK runtime, and follows the selected app theme; unsupported systems retain the translucent UI fallback. Active work appears as a concise status pill with cancellation rather than step-by-step progress. Browser preview includes simulated voice listening and word-by-word, soft-blur speech transcription below the waveform; the finished command fills the composer automatically without a confirmation step. It never accesses a microphone, and actual voice capture remains deferred. The native window is hidden until opened through configurable Ctrl+Shift+Space or the tray; tray settings allow shortcut reassignment and expose registration failures. The window is frameless and transparent, and is positioned near the top center of the invocation monitor. Click-through, focus restoration, DPI, and multi-monitor behavior still require interactive Windows validation.

With Node.js 24+ and a Rust toolchain installed, run the app from the repository root:

```sh
npm ci
npm run dev:app
```

For browser-only visual review, run `npm run dev` and use the floating Prototype controls panel to switch Hidden, Command, Voice, Working, Settings, reversible Remove approval, destructive Deletion approval, Done, Error, and Stopped states. Voice transcription and task progress are simulated; no microphone or desktop actions are used. The panel is development-web-only and is absent from the Tauri app. To run the desktop app during development, use `npm run dev:app`, which starts Tauri and Vite together; the dev URL and Vite bind address are both `127.0.0.1:1420`. Build the web UI with `npm run build`. Windows CI builds `deskact.exe` through Tauri CLI's release build with the custom asset protocol, so it serves the bundled `dist` UI directly and does not need Vite, npm, or a localhost server at runtime. The production app uses an isolated Cargo target directory so integration tests cannot overwrite it with a dev-mode executable. Before upload, CI checks its hash and runs `--verify-bundled-ui` to verify production mode, no development URL, and an embedded `index.html` (not a full WebView launch test). CI publishes the app alone as `deskact-prototype-windows-x64`, plus the optional CLI harness separately as `deskact-dev-harness-windows-x64`; it does not produce an installer. See the [non-destructive HUD task smoke test](docs/implementation/adaptive-island-task-smoke-test.md) for deterministic fixture-mode testing. The fixture adapter is disabled in release builds. Real native-window behavior must still be checked on an interactive Windows desktop.

## Automation tools are installed separately

DeskAct does **not** bundle, download, or update `agent-desktop` or `agent-browser`. Install them yourself from their upstream npm packages. DeskAct checks for the expected CLI version when it needs that capability and provides setup guidance if the tool is missing or incompatible.

### Prerequisite

Install Node.js **24 or later** and npm. This satisfies the current `agent-browser` baseline; `agent-desktop` itself documents Node.js 18 or later.

### Native desktop control — required

In Windows PowerShell:

```powershell
npm install --global agent-desktop@1.1.0
agent-desktop version
agent-desktop status
```

`agent-desktop` is required for native desktop control. If it is missing or incompatible, native tasks are unavailable until you install a supported version. DeskAct will not install it automatically.

### Browser control — optional until Phase 3

Browser automation is best-effort and is not required for native desktop use. When browser support is enabled, install the documented baseline:

```powershell
npm install --global agent-browser@0.41.2
npm list --global agent-browser --depth=0
```

`agent-browser@0.41.2` may download Chrome for Testing separately when managed Chrome is needed:

```powershell
agent-browser install
```

This download is separate from npm installation. Planned Phase 3 browser support targets Microsoft Edge via CDP and is not yet validated. Use only a separately launched, dedicated browser profile—not your everyday profile—and verify the debugging listener is loopback-only and allowed by device policy before attaching. A dedicated profile's cookies and authentication are visible to browser automation; you may sign in to it manually, but DeskAct will not import or export saved session state. When needed for a user-requested task, only minimum relevant page context may be sent to your configured LLM endpoint; cookies, tokens, and credentials are never sent, and page content is treated as untrusted. [agent-browser CDP setup](https://agent-browser.dev/cdp-mode). Helium is exploratory and is not a supported target until the same Windows checks pass. Missing or incompatible `agent-browser` disables browser tasks only; it does not block native DeskAct use. DeskAct will not install the package or browser automatically.

### Troubleshooting

- If PowerShell cannot find a CLI after installation, open a new terminal and check that the npm global bin directory is on `PATH`.
- The browser package declares Node.js `>=24`. Check with `node --version` if installation or execution reports an engine mismatch.
- A TLS `UnknownIssuer` error while downloading Chrome for Testing indicates the machine does not trust the certificate chain. Do not disable TLS verification; resolve the trust configuration with your IT administrator, or use an authorized existing Edge/CDP setup.

## Phase 1 developer harness

The developer-only Rust harness checks the user-installed native CLI, discovers app entries from Start Menu shortcuts and registered App Paths, and lists currently running GUI apps. Its `resolve` command can activate an exact-name discovered or user-configured app, then identify a unique accessible window from fresh window inventory and take a fresh accessibility snapshot; this is a target-resolution smoke check, not a general task executor. The controlled Notepad acceptance command creates a new tab only in exactly one accessible, non-minimized, owner-verified existing Notepad window, saves a fixed marker under a unique temporary directory, verifies both fresh editor readback and file contents, then closes only the tab whose unique saved filename was verified. It preserves existing tabs, does not inspect their document contents, and cleans up only its own test file and directory. A run is verified only after the original tab/window state is observed again. The default 20-run gate requires at least 19 verified runs; failures and partial evidence are reported, and ambiguous UI states fail closed. The File Explorer acceptance command creates a unique temporary folder and text fixture, navigates to it and opens the file using accessible controls, then verifies the folder path and fixture contents in fresh UI snapshots. It uses PowerShell `Start-Process` only to open Explorer when no Explorer window is available; it never passes the fixture path through the launch command. It cleans up only its own file and folder and fails closed if target or opened-file evidence is missing or ambiguous. It does not close the associated text-viewer window because that window may be shared with user documents; the fixture tab may remain open after cleanup. The Calculator acceptance command activates the packaged Calculator app only when no Calculator window is already open, refuses ambiguous windows, and performs `7 × 6` through fresh accessible keypad controls. If the existing display is not clear, it uses the accessible Clear control and verifies the reset; it leaves Calculator open and verifies `42` from a fresh snapshot. The VLC acceptance command requires VLC to be installed, creates a unique silent 30-second WAV and isolated `vlcrc` under a temporary workspace, disables metadata networking and recent-media tracking, and starts a separate VLC instance. It requires both an accessible playing-state indicator and advancing playback time in fresh snapshots; missing or ambiguous evidence fails closed as unverified. It closes only the selected test window with Alt+F4 and stops the launched VLC PID only while its process command line still contains the unique fixture path, then removes the fixture workspace. VLC 2.2.3 exposes no verified command-line option to redirect its separate Qt UI settings store, so those preferences may still be read or modified; use this acceptance command only when that limitation is acceptable. These acceptance workflows do not install tools. Discovered catalog entries are not proof that an app is launchable or exposes usable accessibility controls; discovery reflects only these Windows sources. Start Menu shortcuts without a file target are skipped and their count is reported. The `app-catalog` commands combine discovered entries with user-configured apps persisted locally at `%LOCALAPPDATA%\DeskAct\apps.json` on Windows (`DESKACT_APP_CATALOG` overrides the path). Custom targets must be existing absolute `.exe` files; custom entries do not accept model-provided paths or arbitrary launch arguments.

From a Rust development environment on Windows, run from the repository root:

```powershell
cargo run --manifest-path src-tauri/Cargo.toml -- preflight
cargo run --manifest-path src-tauri/Cargo.toml -- inventory
cargo run --manifest-path src-tauri/Cargo.toml -- resolve Calculator
cargo run --manifest-path src-tauri/Cargo.toml -- app-catalog list
cargo run --manifest-path src-tauri/Cargo.toml -- app-catalog add "My Editor" "C:\Program Files\Vendor\Editor\Editor.exe" --alias editor
# Replace ENTRY_ID with the id printed by app-catalog add; --alias replaces the alias list
cargo run --manifest-path src-tauri/Cargo.toml -- app-catalog edit ENTRY_ID --alias code --alias IDE
cargo run --manifest-path src-tauri/Cargo.toml -- app-catalog prepare "open my editor"
cargo run --manifest-path src-tauri/Cargo.toml -- app-catalog validate-selection "open my editor" --response-json '{"decision":"select","candidate_id":"ENTRY_ID"}'
# Configure the primary Chat Completions endpoint in the environment before testing:
$env:DESKACT_LLM_BASE_URL = "https://provider.example/v1"
$env:DESKACT_LLM_MODEL = "<explicit-model-id>"
# Optional: $env:DESKACT_LLM_API_KEY = "<credential>" (never pass credentials as CLI arguments)
cargo run --manifest-path src-tauri/Cargo.toml -- llm test
cargo run --manifest-path src-tauri/Cargo.toml -- llm plan "open Calculator"
cargo run --manifest-path src-tauri/Cargo.toml -- llm validate-plan --response-json '{"version":1,"summary":"Open Calculator","steps":[{"operation":"open_app","parameters":{"app_id":"ENTRY_ID"},"expected_outcome":"Calculator is open"}]}'
# Open an exact catalog app deterministically, or send a non-exact request to the configured primary model:
cargo run --manifest-path src-tauri/Cargo.toml -- task execute --request "open Calculator"
cargo run --manifest-path src-tauri/Cargo.toml -- task execute --request "open my calculator"
cargo run --manifest-path src-tauri/Cargo.toml -- notepad-acceptance
cargo run --manifest-path src-tauri/Cargo.toml -- file-explorer-acceptance
cargo run --manifest-path src-tauri/Cargo.toml -- calculator-acceptance
cargo run --manifest-path src-tauri/Cargo.toml -- vlc-acceptance
# Deterministic task-recovery policy simulations (perform no native actions):
cargo run --manifest-path src-tauri/Cargo.toml -- task simulate --scenario unknown-delivery
cargo run --manifest-path src-tauri/Cargo.toml -- task simulate --scenario unknown-delivery-achieved
cargo run --manifest-path src-tauri/Cargo.toml -- task simulate --scenario unknown-delivery-safe-retry
cargo run --manifest-path src-tauri/Cargo.toml -- task simulate --scenario stale-reference
cargo run --manifest-path src-tauri/Cargo.toml -- task simulate --scenario incomplete-observation
cargo run --manifest-path src-tauri/Cargo.toml -- task simulate --scenario permission-denied
cargo run --manifest-path src-tauri/Cargo.toml -- task simulate --scenario busy-request
cargo run --manifest-path src-tauri/Cargo.toml -- task simulate --scenario cancel-in-flight
cargo run --manifest-path src-tauri/Cargo.toml -- task simulate --scenario ui-interference
cargo run --manifest-path src-tauri/Cargo.toml -- task simulate --scenario ui-interference-unclear
# Action-specific approval simulations use a recording-only adapter; no files or OS state are changed:
cargo run --manifest-path src-tauri/Cargo.toml -- task approval-simulate --scenario unapproved
cargo run --manifest-path src-tauri/Cargo.toml -- task approval-simulate --scenario approved
cargo run --manifest-path src-tauri/Cargo.toml -- task approval-simulate --scenario stale-action
cargo run --manifest-path src-tauri/Cargo.toml -- task approval-simulate --scenario stale-target
cargo run --manifest-path src-tauri/Cargo.toml -- task approval-simulate --scenario stale-parameters
# Optional short smoke check (does not assess the 19/20 gate):
cargo run --manifest-path src-tauri/Cargo.toml -- notepad-acceptance --runs 1
```

`task approval-simulate` exercises Rust's local risk classification, exact action/target/parameter approval binding, and stale-approval rejection through a recording-only adapter; its fixture models file deletion but never deletes or changes a real file. It does not enable consequential operations in `task execute`, which remains limited to verified app opening. `unapproved` must not dispatch, `approved` records one dispatch, and each `stale-*` scenario changes one part of the approved scope and records no dispatch. If the matching window is minimized, the harness asks you to restore it manually, then re-enumerates windows and takes a fresh snapshot. The Notepad acceptance command requires one already-open, uniquely matched accessible Notepad window; it uses Ctrl+N and Ctrl+W only after fresh state checks and never treats a delivered shortcut/click as proof of success. It never relies on launch output alone to select a window, and stops on missing, ambiguous, inaccessible, or unverified targets. The native CLI must be installed separately as described above. For a non-default npm installation, set `DESKACT_AGENT_DESKTOP` to the underlying executable path; do not point it at a PowerShell script or shell wrapper. Set `DESKACT_POWERSHELL` only when PowerShell is not discoverable by its default executable name. The harness reports discovered app names, targets, and sources, plus running app names, process IDs, and presentation state returned by `agent-desktop`. `app-catalog prepare` emits only candidate IDs and display names for non-exact requests. `validate-selection` accepts a fixture JSON response and validates it against a freshly prepared request and local catalog; it does not call an LLM. The validator rejects IDs outside that candidate set. The `llm test` command reads `DESKACT_LLM_BASE_URL`, `DESKACT_LLM_MODEL`, and optional `DESKACT_LLM_API_KEY`; it sends only a synthetic connectivity request and performs no desktop actions. Use HTTPS for remote endpoints; HTTP is allowed only for loopback. This is a basic connectivity/authentication/response check, not a guarantee that action plans are supported. `llm plan <request>` returns only a complete locally validated JSON action plan; it does not execute actions. A malformed plan gets at most one bounded correction request, then fails closed. `llm validate-plan` validates fixture JSON locally without network access. `task execute --request <request>` uses a deterministic fast path for `open <exact catalog app name or alias>`; non-exact `open ...` requests go to the configured primary model with catalog IDs and display names only. This first executor slice runs only explicit `open <app>` requests and permits only `open_app` with a catalog ID, then verifies a unique accessible window and fresh complete accessibility snapshot. Other intents, unknown operations, ungrounded app IDs, and unsupported expected outcomes fail closed before any launch. The task report records native evidence, achieved postconditions, and unresolved/unattempted steps; it does not treat a model's summary as verification.

The default command runs 20 scenarios and passes only with at least 19 independently verified successes. `--runs` accepts counts from 1 through 100; shorter counts are smoke checks, not reliability-gate results. The Windows CI job uploads a `deskact-dev-windows-x64` artifact for 14 days. This is a developer command-line harness, not an installer or end-user DeskAct app; it still requires the separately installed `agent-desktop` CLI.

## License

DeskAct's source code and documentation are licensed under the [Apache License 2.0](LICENSE). Copyright © 2026 Eguin Jonathan. This project license applies to DeskAct materials; independently installed third-party tools remain under their respective upstream licenses. Third-party components shipped inside a future DeskAct release will retain their own license terms and required notices.

## Upstream package and license information

- [`agent-desktop@1.1.0`](https://www.npmjs.com/package/agent-desktop) · [upstream repository](https://github.com/vercel-labs/agent-desktop) · [Apache-2.0 license](https://github.com/vercel-labs/agent-desktop/blob/v1.1.0/LICENSE)
- [`agent-browser@0.41.2`](https://www.npmjs.com/package/agent-browser) · [upstream repository](https://github.com/vercel-labs/agent-browser) · [Apache-2.0 license](https://github.com/vercel-labs/agent-browser/blob/v0.41.2/LICENSE)
