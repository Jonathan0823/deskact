# DeskAct

DeskAct is a Windows-first, accessibility-first desktop assistant for finding, opening, and controlling supported apps. The product is in the specification and Windows validation stage; the DeskAct application is not yet released. See the [Product Requirements Document](docs/DeskAct_PRD_v1.13.md) and [Glossary](GLOSSARY.md).

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

The developer-only Rust harness checks the user-installed native CLI, discovers app entries from Start Menu shortcuts and registered App Paths, and lists currently running GUI apps. It does not install tools or control apps. Discovery reflects these Windows sources; it is not a claim that every installed application exposes usable accessibility controls.

From a Rust development environment on Windows, run from the repository root:

```powershell
cargo run --manifest-path src-tauri/Cargo.toml -- preflight
cargo run --manifest-path src-tauri/Cargo.toml -- inventory
```

The native CLI must be installed separately as described above. For a non-default npm installation, set `DESKACT_AGENT_DESKTOP` to the underlying executable path; do not point it at a PowerShell script or shell wrapper. Set `DESKACT_POWERSHELL` only when PowerShell is not discoverable by its default executable name. The harness reports discovered app names, targets, and sources, plus running app names, process IDs, and presentation state returned by `agent-desktop`.

## License

DeskAct's source code and documentation are licensed under the [Apache License 2.0](LICENSE). Copyright © 2026 Eguin Jonathan. This project license applies to DeskAct materials; independently installed third-party tools remain under their respective upstream licenses. Third-party components shipped inside a future DeskAct release will retain their own license terms and required notices.

## Upstream package and license information

- [`agent-desktop@1.1.0`](https://www.npmjs.com/package/agent-desktop) · [upstream repository](https://github.com/vercel-labs/agent-desktop) · [Apache-2.0 license](https://github.com/vercel-labs/agent-desktop/blob/v1.1.0/LICENSE)
- [`agent-browser@0.41.2`](https://www.npmjs.com/package/agent-browser) · [upstream repository](https://github.com/vercel-labs/agent-browser) · [Apache-2.0 license](https://github.com/vercel-labs/agent-browser/blob/v0.41.2/LICENSE)
