use tauri::{AppHandle, Emitter};
#[cfg(desktop)]
use tauri::Manager;

use crate::{
    auth::{load_app_settings, save_app_settings},
    types::{AppLanguage, AppSettings},
};

pub const LANGUAGE_CHANGED_EVENT: &str = "language-changed";
pub const SETTINGS_CHANGED_EVENT: &str = "settings-changed";

#[tauri::command]
pub fn get_app_settings() -> AppSettings {
    load_app_settings().unwrap_or_default()
}

#[tauri::command]
pub fn set_app_settings(app: AppHandle, mut settings: AppSettings) -> Result<AppSettings, String> {
    let previous = load_app_settings().unwrap_or_default();
    settings.floating.normalize_modes(Some(&previous.floating));
    #[cfg(desktop)]
    if previous.floating.compact_mode && !settings.floating.compact_mode {
        if let Some(window) = app.get_webview_window(crate::floating::FLOATING_WINDOW) {
            if let Ok(position) = window.outer_position() {
                settings.floating.position = Some((position.x, position.y));
            }
        }
    }
    settings.floating.opacity = settings.floating.opacity.clamp(0.25, 1.0);
    if settings.floating.visible_fields.is_empty() {
        settings.floating.visible_fields = crate::types::FloatingSettings::default().visible_fields;
    }
    save_app_settings(&settings).map_err(|error| error.to_string())?;

    #[cfg(desktop)]
    {
        crate::tray::refresh(&app);
        crate::floating::apply_settings(&app, &settings);
        #[cfg(target_os = "windows")]
        crate::taskbar_widget::apply_settings(&app, &settings);
    }
    let _ = app.emit(SETTINGS_CHANGED_EVENT, settings.clone());
    Ok(settings)
}

#[tauri::command]
pub fn get_app_language() -> AppLanguage {
    load_app_settings().unwrap_or_default().language
}

#[tauri::command]
pub fn set_app_language(app: AppHandle, language: AppLanguage) -> Result<AppLanguage, String> {
    let language = save_language(language)?;

    #[cfg(desktop)]
    {
        if let Err(error) = crate::app_menu::refresh(&app) {
            eprintln!("Failed to refresh app menu after language change: {error}");
        }
        crate::tray::refresh(&app);
        #[cfg(target_os = "windows")]
        {
            let settings = load_app_settings().unwrap_or_default();
            crate::taskbar_widget::apply_settings(&app, &settings);
            let _ = app.emit(SETTINGS_CHANGED_EVENT, settings);
        }
    }

    let _ = app.emit(LANGUAGE_CHANGED_EVENT, language.clone());
    Ok(language)
}

pub fn save_language(language: AppLanguage) -> Result<AppLanguage, String> {
    if !crate::i18n::is_supported(&language) {
        return Err(format!("Unsupported app language: {}", language.as_str()));
    }

    let mut settings = load_app_settings().unwrap_or_default();
    if settings.language != language {
        settings.language = language.clone();
        save_app_settings(&settings).map_err(|error| error.to_string())?;
    }
    Ok(language)
}
