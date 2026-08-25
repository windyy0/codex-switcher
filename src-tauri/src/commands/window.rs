//! Window and tray popup management commands.

use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(target_os = "macos")]
use std::time::Duration;

use tauri::{AppHandle, Manager, Runtime};

#[cfg(target_os = "macos")]
use crate::auth::{load_app_settings, save_app_settings};
use crate::types::{DockDisplayMode, UsageInfo};

pub const CLOSE_BEHAVIOR_REQUESTED_EVENT: &str = "close-behavior-requested";

#[cfg(target_os = "macos")]
static CLOSE_BEHAVIOR_PROMPT_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static CLOSE_BEHAVIOR_PROMPT_ACKED: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseBehaviorRequestedPayload {
    pub request_id: u64,
}

/// Receive the main app's polled usage so the tray menu can show remaining quota
/// without doing its own fetching. The main window is the single usage poller.
#[tauri::command]
pub fn report_usage(app: AppHandle, usages: Vec<UsageInfo>) {
    #[cfg(desktop)]
    crate::tray::ingest_usage(&app, usages);
    #[cfg(not(desktop))]
    let _ = (app, usages);
}

/// Bring the main window to the foreground.
#[tauri::command]
pub fn open_main_window(app: AppHandle) {
    restore_main_window(&app);
}

#[tauri::command]
pub fn close_main_window(app: AppHandle) -> Result<(), String> {
    try_hide_main_window(&app)
}

fn try_hide_main_window<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is not available".to_string())?;
    window
        .hide()
        .map_err(|error| format!("failed to hide main window: {error}"))?;
    #[cfg(target_os = "macos")]
    app.hide()
        .map_err(|error| format!("failed to hide application: {error}"))?;
    Ok(())
}

pub fn hide_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Err(error) = try_hide_main_window(app) {
        eprintln!("Failed to hide main window: {error}");
    }
}

/// Hide after the current native close callback has returned.
///
/// On Windows, changing visibility synchronously inside `CloseRequested` is
/// re-entrant in the wry event loop and can still destroy the native window
/// after `prevent_close()`. Sending the visibility change from a worker queues
/// it for the next event-loop turn.
pub fn schedule_hide_main_window<R: Runtime>(app: AppHandle<R>) {
    let app_handle = app.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("main-window-hide-dispatch".into())
        .spawn(move || {
            let hide_app = app_handle.clone();
            if let Err(error) = app_handle.run_on_main_thread(move || {
                hide_main_window(&hide_app);
            }) {
                eprintln!("Failed to schedule main window hide: {error}");
            }
        })
    {
        eprintln!("Failed to start main window hide dispatcher: {error}");
    }
}

#[cfg(target_os = "macos")]
pub fn next_close_behavior_prompt_payload() -> CloseBehaviorRequestedPayload {
    CloseBehaviorRequestedPayload {
        request_id: CLOSE_BEHAVIOR_PROMPT_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1,
    }
}

#[cfg(target_os = "macos")]
pub fn schedule_close_behavior_prompt_fallback<R: Runtime>(app: AppHandle<R>, request_id: u64) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(750));
        if CLOSE_BEHAVIOR_PROMPT_ACKED.load(Ordering::SeqCst) >= request_id {
            return;
        }

        let app_handle = app.clone();
        if let Err(error) = app.run_on_main_thread(move || {
            hide_main_window(&app_handle);
        }) {
            eprintln!("Failed to schedule close prompt fallback: {error}");
        }
    });
}

/// Bring the main window to the foreground.
pub fn restore_main_window<R: Runtime>(app: &AppHandle<R>) {
    #[cfg(target_os = "macos")]
    let _ = app.show();
    let window = app.get_webview_window("main").or_else(|| {
        let config = app
            .config()
            .app
            .windows
            .iter()
            .find(|config| config.label == "main")?;
        match tauri::WebviewWindowBuilder::from_config(app, config)
            .and_then(|builder| builder.build())
        {
            Ok(window) => Some(window),
            Err(error) => {
                eprintln!("Failed to recreate main window: {error}");
                None
            }
        }
    });
    let Some(window) = window else {
        eprintln!("Failed to restore main window: no main window configuration");
        return;
    };
    if let Err(error) = window.show() {
        eprintln!("Failed to show main window: {error}");
    }
    if let Err(error) = window.unminimize() {
        eprintln!("Failed to unminimize main window: {error}");
    }
    if let Err(error) = window.set_focus() {
        eprintln!("Failed to focus main window: {error}");
    }
}

#[tauri::command]
pub fn get_dock_display_mode() -> Option<DockDisplayMode> {
    #[cfg(target_os = "macos")]
    {
        Some(
            crate::auth::load_app_settings()
                .unwrap_or_default()
                .dock_display_mode,
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[tauri::command]
pub fn set_dock_display_mode(
    app: AppHandle,
    mode: DockDisplayMode,
) -> Result<Option<DockDisplayMode>, String> {
    #[cfg(target_os = "macos")]
    {
        crate::app_menu::set_dock_display_mode(&app, mode)
            .map(|settings| Some(settings.dock_display_mode))
            .map_err(|error| error.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, mode);
        Ok(None)
    }
}

#[tauri::command]
pub fn complete_close_behavior(
    app: AppHandle,
    mode: DockDisplayMode,
    dont_ask_again: bool,
) -> Result<Option<DockDisplayMode>, String> {
    #[cfg(target_os = "macos")]
    {
        let mut settings = crate::app_menu::set_dock_display_mode(&app, mode)
            .map_err(|error| error.to_string())?;
        if dont_ask_again {
            settings.close_behavior_prompt_enabled = false;
            save_app_settings(&settings).map_err(|error| error.to_string())?;
        }
        hide_main_window(&app);
        Ok(Some(settings.dock_display_mode))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (mode, dont_ask_again);
        hide_main_window(&app);
        Ok(None)
    }
}

#[tauri::command]
pub fn ack_close_behavior_prompt(request_id: u64) {
    CLOSE_BEHAVIOR_PROMPT_ACKED.fetch_max(request_id, Ordering::SeqCst);
}

pub fn should_prompt_for_close_behavior() -> bool {
    #[cfg(target_os = "macos")]
    {
        load_app_settings()
            .unwrap_or_default()
            .close_behavior_prompt_enabled
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}
