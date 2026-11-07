# Native Island Window Probe (throwaway)

This Windows-only probe answers one question: can a top-level Win32 window use Wisp's Acrylic composition path and a native rounded region without a caption, including while inactive?

It is deliberately separate from the DeskAct app and does not use Tauri, WebView2, HTML, or screen capture. It creates two borderless native HWNDs, draws their surfaces with Direct2D, and applies the Acrylic composition policy plus a rounded Win32 region: a compact island and an independently sized Settings surface. The app's Rust orchestration and accessibility are not part of this probe; it is only a material/window-host experiment.

## Run on Windows

```powershell
cargo run --release --manifest-path prototypes/native-island/Cargo.toml
```

Or download `deskact-native-island-probe-windows-x64` from the `Native Island Window Probe` GitHub Actions run for this branch.

## Manual checks

1. Inspect the island while focused and after clicking another app. The in-surface heading is expected; there should be no OS titlebar or caption outside the rounded surface in either state.
2. Check all four corners against a detailed desktop background. Acrylic must stop at the island's capsule outline; no square material should remain outside it.
3. Press **F1** to show the separate, larger Settings surface. Check its rounded corners and confirm the island window's size is unaffected.
4. Click another app while Settings is visible and verify Acrylic remains present.
5. Press **Esc** to return from Settings; press **Esc** again to quit.

`SetWindowCompositionAttribute` is an undocumented Windows API, used here only as a throwaway experiment. A successful compile is not a visual pass: these checks must be run on the target Windows/WebView-independent desktop before adopting the approach in DeskAct.
