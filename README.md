# DeskAct

DeskAct is a Windows-first, accessibility-first desktop assistant for finding, opening, and controlling supported apps. The product is in the specification and Windows validation stage; the DeskAct application is not yet released. See the [Product Requirements Document](docs/DeskAct_PRD_v1.13.md), [Glossary](GLOSSARY.md), and proposed [Adaptive Island UI/UX spec](design/ui/DeskAct_Adaptive_Island_UI_Spec_v0.1.md). The UI spec is a text proposal, not a visual mockup or implemented application.

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

The developer-only Rust harness checks the user-installed native CLI, discovers app entries from Start Menu shortcuts and registered App Paths, and lists currently running GUI apps. Its `resolve` command can activate a discovered app, then identify a unique accessible window from fresh window inventory and take a fresh accessibility snapshot; this is a target-resolution smoke check, not a general task executor. The controlled Notepad acceptance command creates a new tab only in exactly one accessible, non-minimized, owner-verified existing Notepad window, saves a fixed marker under a unique temporary directory, verifies both fresh editor readback and file contents, then closes only the tab whose unique saved filename was verified. It preserves existing tabs, does not inspect their document contents, and cleans up only its own test file and directory. A run is verified only after the original tab/window state is observed again. The default 20-run gate requires at least 19 verified runs; failures and partial evidence are reported, and ambiguous UI states fail closed. The File Explorer acceptance command creates a unique temporary folder and text fixture, navigates to it and opens the file using accessible controls, then verifies the folder path and fixture contents in fresh UI snapshots. It uses PowerShell `Start-Process` only to open Explorer when no Explorer window is available; it never passes the fixture path through the launch command. It cleans up only its own file and folder and fails closed if target or opened-file evidence is missing or ambiguous. It does not close the associated text-viewer window because that window may be shared with user documents; the fixture tab may remain open after cleanup. It does not install tools. Catalog entries are shown as discovered, not as proof they are launchable or expose usable accessibility controls; discovery reflects only these Windows sources. Start Menu shortcuts without a file target are skipped and their count is reported.

From a Rust development environment on Windows, run from the repository root:

```powershell
cargo run --manifest-path src-tauri/Cargo.toml -- preflight
cargo run --manifest-path src-tauri/Cargo.toml -- inventory
cargo run --manifest-path src-tauri/Cargo.toml -- resolve Calculator
cargo run --manifest-path src-tauri/Cargo.toml -- notepad-acceptance
cargo run --manifest-path src-tauri/Cargo.toml -- file-explorer-acceptance
# Optional short smoke check (does not assess the 19/20 gate):
cargo run --manifest-path src-tauri/Cargo.toml -- notepad-acceptance --runs 1
```

If the matching window is minimized, the harness asks you to restore it manually, then re-enumerates windows and takes a fresh snapshot. The Notepad acceptance command requires one already-open, uniquely matched accessible Notepad window; it uses Ctrl+N and Ctrl+W only after fresh state checks and never treats a delivered shortcut/click as proof of success. It never relies on launch output alone to select a window, and stops on missing, ambiguous, inaccessible, or unverified targets. The native CLI must be installed separately as described above. For a non-default npm installation, set `DESKACT_AGENT_DESKTOP` to the underlying executable path; do not point it at a PowerShell script or shell wrapper. Set `DESKACT_POWERSHELL` only when PowerShell is not discoverable by its default executable name. The harness reports discovered app names, targets, and sources, plus running app names, process IDs, and presentation state returned by `agent-desktop`.

The default command runs 20 scenarios and passes only with at least 19 independently verified successes. `--runs` accepts counts from 1 through 100; shorter counts are smoke checks, not reliability-gate results. The Windows CI job uploads a `deskact-dev-windows-x64` artifact for 14 days. This is a developer command-line harness, not an installer or end-user DeskAct app; it still requires the separately installed `agent-desktop` CLI.

## License

DeskAct's source code and documentation are licensed under the [Apache License 2.0](LICENSE). Copyright © 2026 Eguin Jonathan. This project license applies to DeskAct materials; independently installed third-party tools remain under their respective upstream licenses. Third-party components shipped inside a future DeskAct release will retain their own license terms and required notices.

## Upstream package and license information

- [`agent-desktop@1.1.0`](https://www.npmjs.com/package/agent-desktop) · [upstream repository](https://github.com/vercel-labs/agent-desktop) · [Apache-2.0 license](https://github.com/vercel-labs/agent-desktop/blob/v1.1.0/LICENSE)
- [`agent-browser@0.41.2`](https://www.npmjs.com/package/agent-browser) · [upstream repository](https://github.com/vercel-labs/agent-browser) · [Apache-2.0 license](https://github.com/vercel-labs/agent-browser/blob/v0.41.2/LICENSE)
