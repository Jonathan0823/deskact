mod shortcuts;
mod tasks;
mod tray;
mod window_behavior;

use shortcuts::{ShortcutState, ShortcutStatus};
use tasks::TaskHub;
use tauri::Manager;
use tauri::window::{Effect, EffectsBuilder};

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
            quit_application,
            dismiss_island
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                window_behavior::request_quit(window.app_handle());
            }
        })
        .setup(|app| {
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
fn set_backdrop_material(
    window: tauri::WebviewWindow,
    enabled: bool,
    dark: bool,
) -> Result<(), String> {
    let effects = enabled.then(|| {
        let effect = if dark {
            Effect::MicaDark
        } else {
            Effect::MicaLight
        };
        EffectsBuilder::new().effect(effect).build()
    });
    window
        .set_effects(effects)
        .map_err(|error| format!("Could not update the native backdrop effect: {error}"))
}

#[tauri::command]
fn quit_application(app: tauri::AppHandle) {
    window_behavior::quit_application(&app);
}

#[tauri::command]
fn dismiss_island(app: tauri::AppHandle) -> Result<(), String> {
    window_behavior::dismiss(&app)
}
