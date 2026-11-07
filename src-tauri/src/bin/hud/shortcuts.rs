use std::{fs, path::PathBuf, str::FromStr, sync::Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, path::BaseDirectory};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

const DEFAULT_SHORTCUT: &str = "Ctrl+Shift+Space";
const SETTINGS_FILE: &str = "adaptive-island.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShortcutStatus {
    pub accelerator: String,
    pub registered: bool,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct ShortcutState(Mutex<Option<ShortcutStatus>>);

impl ShortcutState {
    pub fn snapshot(&self) -> ShortcutStatus {
        self.0
            .lock()
            .expect("shortcut state poisoned")
            .clone()
            .unwrap_or_else(|| ShortcutStatus {
                accelerator: DEFAULT_SHORTCUT.to_owned(),
                registered: false,
                error: Some("Shortcut registration has not completed".to_owned()),
            })
    }

    fn replace(&self, status: ShortcutStatus) {
        *self.0.lock().expect("shortcut state poisoned") = Some(status);
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct ShortcutSettings {
    accelerator: String,
}

pub fn install_shortcut(app: &AppHandle, state: &ShortcutState) {
    let (accelerator, read_error) = load_shortcut(app);
    let registration = parse_shortcut(&accelerator)
        .map_err(|error| error.to_string())
        .and_then(|shortcut| {
            app.global_shortcut()
                .register(shortcut)
                .map_err(|error| error.to_string())
        });

    let status = match registration {
        Ok(()) => ShortcutStatus {
            accelerator,
            registered: true,
            error: read_error,
        },
        Err(error) => ShortcutStatus {
            accelerator,
            registered: false,
            error: Some(format!(
                "Could not register the global shortcut. Use tray settings to choose another. {error}"
            )),
        },
    };
    state.replace(status);
}

pub fn update_global_shortcut(
    app: &AppHandle,
    state: &ShortcutState,
    accelerator: String,
) -> Result<ShortcutStatus, String> {
    let shortcut = parse_shortcut(&accelerator)?;
    let previous = state.snapshot();
    if previous.registered && parse_shortcut(&previous.accelerator).ok() == Some(shortcut) {
        return Ok(previous);
    }

    app.global_shortcut()
        .register(shortcut)
        .map_err(|error| format!("Could not register `{accelerator}`: {error}"))?;

    if let Err(error) = save_shortcut(app, &accelerator) {
        let _ = app.global_shortcut().unregister(shortcut);
        return Err(error);
    }

    if previous.registered {
        if let Ok(previous_shortcut) = parse_shortcut(&previous.accelerator) {
            if let Err(error) = app.global_shortcut().unregister(previous_shortcut) {
                let _ = app.global_shortcut().unregister(shortcut);
                let _ = save_shortcut(app, &previous.accelerator);
                let status = ShortcutStatus {
                    error: Some(format!("Could not release the previous shortcut: {error}")),
                    ..previous
                };
                state.replace(status.clone());
                return Err(status.error.expect("error was set"));
            }
        }
    }

    let status = ShortcutStatus {
        accelerator,
        registered: true,
        error: None,
    };
    state.replace(status.clone());
    Ok(status)
}

fn parse_shortcut(accelerator: &str) -> Result<Shortcut, String> {
    let trimmed = accelerator.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return Err("Enter a valid shortcut such as Ctrl+Shift+Space".to_owned());
    }
    Shortcut::from_str(trimmed)
        .map_err(|_| "Enter a valid shortcut such as Ctrl+Shift+Space".to_owned())
}

fn load_shortcut(app: &AppHandle) -> (String, Option<String>) {
    let path = match settings_path(app) {
        Ok(path) => path,
        Err(error) => return (DEFAULT_SHORTCUT.to_owned(), Some(error)),
    };
    match fs::read_to_string(path) {
        Ok(contents) => match serde_json::from_str::<ShortcutSettings>(&contents) {
            Ok(settings) => match parse_shortcut(&settings.accelerator) {
                Ok(_) => (settings.accelerator, None),
                Err(_) => (
                    DEFAULT_SHORTCUT.to_owned(),
                    Some("Saved shortcut is invalid; using Ctrl+Shift+Space".to_owned()),
                ),
            },
            Err(_) => (
                DEFAULT_SHORTCUT.to_owned(),
                Some("Shortcut settings could not be read; using Ctrl+Shift+Space".to_owned()),
            ),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            (DEFAULT_SHORTCUT.to_owned(), None)
        }
        Err(_) => (
            DEFAULT_SHORTCUT.to_owned(),
            Some("Shortcut settings could not be read; using Ctrl+Shift+Space".to_owned()),
        ),
    }
}

fn save_shortcut(app: &AppHandle, accelerator: &str) -> Result<(), String> {
    let path = settings_path(app)?;
    let parent = path
        .parent()
        .ok_or_else(|| "Could not determine the shortcut settings directory".to_owned())?;
    fs::create_dir_all(parent).map_err(|_| "Could not create the settings directory".to_owned())?;
    let serialized = serde_json::to_vec(&ShortcutSettings {
        accelerator: accelerator.to_owned(),
    })
    .map_err(|_| "Could not serialize shortcut settings".to_owned())?;
    fs::write(path, serialized).map_err(|_| "Could not save shortcut settings".to_owned())
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .resolve(SETTINGS_FILE, BaseDirectory::AppConfig)
        .map_err(|_| "Could not determine the settings path".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_shortcut_is_a_valid_global_accelerator() {
        assert!(parse_shortcut(DEFAULT_SHORTCUT).is_ok());
    }

    #[test]
    fn malformed_and_empty_shortcuts_are_rejected() {
        assert!(parse_shortcut("").is_err());
        assert!(parse_shortcut("Ctrl+DefinitelyNotAKey").is_err());
        assert!(parse_shortcut("Ctrl+\nSpace").is_err());
    }
}
