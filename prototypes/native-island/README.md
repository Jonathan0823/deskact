# Native Island Window Probe (throwaway)

This Windows-only probe asks whether a captionless top-level Win32 window can display a Windows.UI.Composition host-backdrop brush inside a rounded Composition clip and matching native window region, including while inactive.

It is deliberately separate from the DeskAct app and does not use Tauri, WebView2, HTML, or screen capture. It creates two native `WS_POPUP` HWNDs, attaches a host-backdrop Composition visual clipped to a rounded rectangle, and draws only Direct2D text/borders over it: a compact island and an independently sized Settings surface. It uses `DWMWA_USE_HOSTBACKDROPBRUSH`, not the undocumented `SetWindowCompositionAttribute` Acrylic accent API. The app's Rust orchestration and accessibility are not part of this probe; it is only a material/window-host experiment.

## Run on Windows

```powershell
cargo run --release --manifest-path prototypes/native-island/Cargo.toml
```

Or download `deskact-native-island-probe-windows-x64` from the `Native Island Window Probe` GitHub Actions run for this branch.

## Manual checks

1. Inspect the island while focused and after clicking another app. The in-surface heading is expected; there must be no OS titlebar or caption outside the rounded surface in either state.
2. Check all four corners against a detailed desktop background. The host backdrop must stop at the island's capsule clip and matching native region; no rectangular material or square halo may remain.
3. Press **F1** to show the separate, larger Settings surface. Check its independent rounded clip and confirm the island window's size is unaffected.
4. Click another app while Settings is visible and verify the host backdrop remains present.
5. Press **Esc** to return from Settings; press **Esc** again to quit.

A successful compile is not a visual pass: these checks must be run on the target Windows desktop before adopting this approach in DeskAct.
