# Native Island Window Probe (throwaway)

This Windows-only probe tests the official Windows App SDK `DesktopAcrylicController` on captionless native Win32 windows, with a matching rounded native window region and an independently sized Settings surface. The explicit `SetWindowRgn` is intentional: the island needs a capsule shape beyond the system's standard rounded-corner preference.

It is deliberately separate from the DeskAct app: no Tauri, WebView2, HTML, or screen capture. It uses a small C++/WinRT shim to initialize the Windows App SDK and attach Desktop Acrylic to each HWND. Diagnostic text and borders are rendered in a click-through transparent overlay so they remain visible above the material. The probe is only a material/window-host experiment; DeskAct's Rust orchestration and accessibility are not involved.

## Run on Windows

Install the **Windows App Runtime 2.6.5 x64** and Visual C++ Redistributable if they are not already present. The runtime installer is linked from Microsoft's [Windows App SDK downloads](https://learn.microsoft.com/en-us/windows/apps/windows-app-sdk/downloads).

The CI artifact is self-contained with respect to the probe executable, shim, and bootstrapper. Extract all files into one folder and run `deskact-native-island-probe.exe`. Alternatively, build locally from a Visual Studio Developer PowerShell with the Windows App SDK 2.6.5 package available:

```powershell
msbuild prototypes/native-island/acrylic-shim/DeskActAcrylicShim.vcxproj /restore /p:Configuration=Release /p:Platform=x64
cargo build --release --manifest-path prototypes/native-island/Cargo.toml --target x86_64-pc-windows-msvc
```

## Manual checks

1. Inspect the island while focused and after clicking another app. The heading and instructions should remain visible over Acrylic; there must be no OS titlebar or caption.
2. Check the capsule edges against a detailed desktop background. Acrylic must stop at the native rounded region, with no rectangular material or square halo outside it.
3. Press **F1** to show the independent Settings window. Check its rounded material and confirm the island window is unchanged.
4. Click another app while Settings is visible and verify the Acrylic material remains present.
5. Press **Esc** to return from Settings; press **Esc** again to quit.

A successful compile is not a visual pass. Confirm the Windows App SDK runtime is installed and perform these checks on the target Windows desktop before adopting this approach in DeskAct.
