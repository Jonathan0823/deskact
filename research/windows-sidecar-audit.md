# Windows sidecar distribution audit: agent-desktop and agent-browser

Context: [Wayfinder research issue #3](https://github.com/Jonathan0823/deskact/issues/3). Audit requested for Windows 11 Pro x64 after global npm installation of `agent-desktop@1.1.0` and `agent-browser@0.41.2`; `agent-browser install` failed fetching Chrome metadata with TLS `UnknownIssuer`. Browser/CDP validation is deferred to Phase 3. This is a distribution/runtime/licensing review, not a recommendation that the tools work correctly in the target environment.

**Evidence cutoff:** sources retrieved 2026-11-04. Versions below are the versions requested, not claims about current latest releases.

## Executive summary

Both upstream projects publish Apache-2.0-identified npm packages and native Windows x64 artifacts. `agent-desktop@1.1.0` documents that its npm package includes native binaries and platform helpers; the package metadata page shows zero runtime dependencies. `agent-browser@0.41.2` is a large platform-binary package with a postinstall script, Node >=24 requirement, and an explicit separate first-run browser download (`agent-browser install`) from Chrome for Testing. Thus npm installation and browser provisioning are distinct steps; the observed TLS failure concerns that latter download, not evidence that npm package installation or browser execution succeeded.

The available registry/version pages and release docs establish top-level licensing and intended contents but do not by themselves establish a complete third-party notice inventory for bundled native dependencies. Before redistributing either package inside a product installer, preserve the exact versioned tarball, enumerate its contents and embedded licenses/notices, and review the relevant upstream license/NOTICE files and native dependency licenses. Do not treat the repository's SPDX field as proof that every bundled component is under the same license.

## Findings

### 1. agent-desktop@1.1.0

- **Package identity and source:** npm identifies version `1.1.0`, Apache-2.0, repository `vercel-labs/agent-desktop`, and upstream homepage. [Versioned npm package page](https://registry.npmjs.org/agent-desktop/1.1.0)
- **Windows payload / runtime:** the package README states that the npm package includes native binaries and platform helpers for x64 and ARM64 and supports Windows; install is `npm install -g agent-desktop`. It says Windows requires no permission grant for applications at the terminal's integrity level. This describes upstream intent, not an independent test of the exact Windows tarball. [Versioned npm package page](https://registry.npmjs.org/agent-desktop/1.1.0)
- **Release/source structure:** the upstream `v1.1.0` tag contains `npm/`, Rust crates/source, a root `LICENSE`, and `Cargo.lock`; the README describes direct release archives as an alternative, requiring the helper beside the executable. [v1.1.0 source tree](https://github.com/vercel-labs/agent-desktop/tree/v1.1.0)
- **Dependencies:** npm's version page reports zero dependencies. This is evidence about declared npm dependencies, not transitive Rust/C/C++ dependencies embedded in the native executable/helpers. [Versioned npm package page](https://registry.npmjs.org/agent-desktop/1.1.0)
- **Notices/licensing:** package metadata and README state Apache-2.0; source tree has a root `LICENSE`. The retrieved sources do not establish whether each published binary/helper carries third-party notices or whether the npm archive includes a full notices bundle. Confirm inside the actual versioned tarball and inspect Rust dependency licensing before repackaging.
- **Installer implications:** npm is the documented cross-platform distribution path; no official default Scoop package was available per README at this version's source snapshot. For a standalone Windows installer, use the matching Windows release archive only after verifying its asset name, checksum/signature, helper placement, and license notices. Do not infer that an npm global install is a self-contained application installer.

### 2. agent-browser@0.41.2

- **Package identity and metadata:** versioned npm metadata specifies Apache-2.0, `bin/agent-browser.js`, `postinstall: node scripts/postinstall.js`, Node `>=24.0.0`, pnpm `>=11.0.0`, npm tarball URL, SHA-512 integrity, 45 files, and unpacked size 121,452,053 bytes. This gives an exact integrity value for pinning/verifying this package artifact. [Versioned npm metadata](https://registry.npmjs.org/agent-browser/0.41.2)
- **Windows payload:** the `v0.41.2` source/tag README says native binaries ship for Windows x64 (as well as macOS/Linux variants) and that npm selects the host binary. [v0.41.2 source tree](https://github.com/vercel-labs/agent-browser/tree/v0.41.2)
- **Runtime browser download:** upstream installation docs explicitly separate global npm installation from `agent-browser install`, which downloads Chrome for Testing on first use. The README identifies Chrome for Testing as the default browser and documents attaching to a running Chrome as an alternate mode. Therefore npm package redistribution alone does not establish that Chrome is included. [Installation docs](https://agent-browser.dev/installation) [v0.41.2 source tree](https://github.com/vercel-labs/agent-browser/tree/v0.41.2)
- **Dependencies:** npm metadata lists no declared `dependencies` field, but does declare a postinstall script and the package is 121 MB unpacked. The `v0.41.2` source tree shows Rust CLI, launcher/bin, scripts, and platform build process. Do not equate the absent npm dependency field with no native or bundled dependencies. [Versioned npm metadata](https://registry.npmjs.org/agent-browser/0.41.2) [v0.41.2 source tree](https://github.com/vercel-labs/agent-browser/tree/v0.41.2)
- **Licensing/notices:** npm metadata and upstream source README identify Apache-2.0; the tagged repository has a root `LICENSE`. The sources retrieved here do not prove complete license/notice coverage of all native components in the npm tarball. Review package contents and Rust dependency licenses/notices before bundling. [Versioned npm metadata](https://registry.npmjs.org/agent-browser/0.41.2) [v0.41.2 source tree](https://github.com/vercel-labs/agent-browser/tree/v0.41.2)
- **Installer implications:** the official npm path entails Node runtime and, for the documented global install, a separate Chrome-for-Testing download. A product installer must decide whether to require external Node/npm and network access, bundle the native CLI plus a legally redistributable browser payload, or provision Chrome separately. Chrome for Testing's redistribution and update terms must be independently checked against its official vendor terms; this audit did not validate bundling Chrome.

## Comparison / practical constraints

| Area | agent-desktop 1.1.0 | agent-browser 0.41.2 |
|---|---|---|
| npm license field | Apache-2.0 | Apache-2.0 |
| Windows x64 artifact | README says native binaries + platform helpers in package | README says native Windows x64 binary selected by npm |
| External runtime | npm/Node launcher path documented; exact Windows minimum Node version not established by retrieved sources | Node >=24 explicitly required by npm metadata |
| First-run external download | No browser download documented as required | Chrome for Testing download is a separate required first-run setup for default browser |
| npm package size / declared deps | version page reports 0 dependencies; exact tarball inventory not captured here | 121,452,053 bytes unpacked, 45 files; no declared dependencies field; postinstall present |
| Main packaging caveat | validate helper/binary pair and bundled Rust dependency notices | separately provision browser, account for Node 24, inspect large native package and browser licensing |

## What was and was not verified

**Checked:** versioned npm metadata/package README for both exact versions; upstream GitHub source trees tagged `v1.1.0` and `v0.41.2`; official agent-browser installation page. The agent-browser npm metadata includes a tarball URL and integrity hash. The agent-desktop registry page reported zero npm dependencies.

**Not checked:** downloaded/extracted npm tarballs; their full file manifests and included license/NOTICE files; hashes/signatures for agent-desktop release assets; full transitive native dependency license inventories; Chrome for Testing vendor redistribution terms; actual Node versions and Windows installer behavior on a clean Windows 11 machine; TLS trust configuration or a network/CDP/browser launch test. No browser/CDP conclusion is implied.

## Recommendation for the distribution decision

Treat both as candidate upstream tools, not yet cleared redistributable bundles. For an internal prototype, use the exact global npm versions and keep browser provisioning separate. For an end-user Windows package, pin and archive each exact npm tarball and its integrity metadata; inspect/extract the Windows-specific payload and notices; obtain upstream's full dependency-license data; and make an explicit Node strategy. For `agent-browser`, separately decide whether to rely on per-user Chrome for Testing download or ship/provision a browser after legal and operational review. The `UnknownIssuer` report is a transport trust failure during browser metadata retrieval; it does not establish a product packaging defect or an upstream license issue.

## Primary sources

- [Issue #3](https://github.com/Jonathan0823/deskact/issues/3) — originating Wayfinder research ticket.
- [agent-desktop@1.1.0 npm registry record](https://registry.npmjs.org/agent-desktop/1.1.0) — package README, license, dependency summary.
- [agent-desktop v1.1.0 upstream source](https://github.com/vercel-labs/agent-desktop/tree/v1.1.0) — tagged source/release structure.
- [agent-browser@0.41.2 npm registry record](https://registry.npmjs.org/agent-browser/0.41.2) — exact metadata, tarball URL/integrity, engines, scripts, size.
- [agent-browser v0.41.2 upstream source](https://github.com/vercel-labs/agent-browser/tree/v0.41.2) — tagged README/source and platform claims.
- [agent-browser installation documentation](https://agent-browser.dev/installation) — official install and browser provisioning steps.
- [agent-browser Apache-2.0 license](https://github.com/vercel-labs/agent-browser/blob/v0.41.2/LICENSE) — tagged license text.
- [agent-desktop Apache-2.0 license](https://github.com/vercel-labs/agent-desktop/blob/v1.1.0/LICENSE) — tagged license text.
