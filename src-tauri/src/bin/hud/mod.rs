mod shortcuts;
mod tasks;
mod tray;
mod window_behavior;
#[cfg(windows)]
mod windows_backdrop;
#[cfg(windows)]
mod windows_frame;

use shortcuts::{ShortcutState, ShortcutStatus};
use tasks::TaskHub;
use tauri::{Emitter, Manager};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppearanceSettings {
    theme: String,
    opacity: u8,
    mica_enabled: bool,
}

pub fn run() {
    let context = tauri::generate_context!();
    if std::env::args().any(|arg| arg == "--verify-bundled-ui") {
        let valid = !tauri::is_dev()
            && context.config().build.dev_url.is_none()
            && context.assets().get(&"index.html".into()).is_some();
        std::process::exit(if valid { 0 } else { 1 });
    }
    tauri::Builder::default()
        .manage(ShortcutState::default())
        .manage(window_behavior::PreviousForegroundWindow::default())
        .manage(window_behavior::WindowMorphGeneration::default())
        .manage(TaskHub::default())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _, event| {
                    if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        window_behavior::open_composer(app);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            get_shortcut_status,
            update_global_shortcut,
            tasks::get_task_snapshot,
            tasks::get_last_task_result,
            tasks::submit_task,
            tasks::stop_task,
            yield_focus,
            morph_island_window,
            set_backdrop_material,
            set_main_backdrop_material,
            publish_appearance,
            hide_settings,
            set_backdrop_visibility,
            quit_application,
            dismiss_island
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if window.label() == "settings" {
                    let _ = window.hide();
                } else {
                    window_behavior::request_quit(window.app_handle());
                }
            }
        })
        .setup(|app| {
            #[cfg(windows)]
            {
                let window = app.get_webview_window("main").ok_or_else(|| {
                    std::io::Error::other("DeskAct's island window is unavailable")
                })?;
                let hwnd = window.hwnd()?;
                // setup runs on the HWND's owning event-loop thread, before show.
                unsafe { windows_frame::install(hwnd.0)? };
            }
            let state = app.state::<ShortcutState>();
            shortcuts::install_shortcut(app.handle(), state.inner());
            tray::install(app)?;
            Ok(())
        })
        .run(context)
        .expect("error while running DeskAct");
}

#[tauri::command]
fn get_shortcut_status(state: tauri::State<'_, ShortcutState>) -> ShortcutStatus {
    state.snapshot()
}

#[tauri::command]
fn update_global_shortcut(
    app: tauri::AppHandle,
    state: tauri::State<'_, ShortcutState>,
    accelerator: String,
) -> Result<ShortcutStatus, String> {
    shortcuts::update_global_shortcut(&app, state.inner(), accelerator)
}

#[tauri::command]
fn yield_focus(app: tauri::AppHandle) {
    window_behavior::yield_focus(&app);
}

#[tauri::command]
async fn morph_island_window(
    app: tauri::AppHandle,
    width: f64,
    height: f64,
    corner_radius: f64,
    reduced_motion: bool,
) -> Result<(), String> {
    window_behavior::morph_window(&app, width, height, corner_radius, reduced_motion).await
}

#[tauri::command]
fn set_backdrop_material(window: tauri::WebviewWindow, enabled: bool) -> Result<(), String> {
    apply_backdrop_material(&window, enabled)
}

#[tauri::command]
fn publish_appearance(app: tauri::AppHandle, appearance: AppearanceSettings) -> Result<(), String> {
    if !matches!(appearance.theme.as_str(), "light" | "dark" | "system")
        || !(18..=85).contains(&appearance.opacity)
    {
        return Err("Appearance settings are outside the supported range".to_owned());
    }
    app.emit_to("main", "deskact://appearance-updated", appearance)
        .map_err(|error| format!("Could not update the island appearance: {error}"))
}

#[tauri::command]
fn set_main_backdrop_material(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "DeskAct's island window is unavailable".to_owned())?;
    apply_backdrop_material(&window, enabled)
}

fn apply_backdrop_material(window: &tauri::WebviewWindow, enabled: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        let hwnd = window
            .hwnd()
            .map_err(|_| "Could not access DeskAct's native window handle".to_owned())?;
        let radius = windows_frame::corner_radius(hwnd.0);
        let visible = window.is_visible().unwrap_or(false);
        return unsafe { windows_backdrop::set_material(hwnd.0, enabled, visible, radius) };
    }
    #[cfg(not(windows))]
    {
        let _ = (window, enabled);
        Ok(())
    }
}

#[tauri::command]
fn hide_settings(app: tauri::AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("settings")
        .ok_or_else(|| "DeskAct's settings window is unavailable".to_owned())?;
    window
        .hide()
        .map_err(|_| "Could not hide DeskAct settings".to_owned())
}

#[tauri::command]
fn set_backdrop_visibility(
    window: tauri::WebviewWindow,
    visible: bool,
    duration_ms: u32,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        let hwnd = window
            .hwnd()
            .map_err(|_| "Could not access DeskAct's native window handle".to_owned())?;
        return windows_backdrop::animate_visibility(hwnd.0, visible, duration_ms);
    }
    #[cfg(not(windows))]
    {
        let _ = (window, visible, duration_ms);
        Ok(())
    }
}

#[tauri::command]
fn quit_application(app: tauri::AppHandle) {
    window_behavior::quit_application(&app);
}

#[tauri::command]
fn dismiss_island(app: tauri::AppHandle) -> Result<(), String> {
    window_behavior::dismiss(&app)
}
