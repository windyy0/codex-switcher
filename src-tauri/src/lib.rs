//! Codex Switcher - Multi-account manager for Codex CLI

pub mod api;
#[cfg(desktop)]
pub mod app_menu;
pub mod auth;
pub mod commands;
#[cfg(desktop)]
pub mod floating;
pub mod i18n;
#[cfg(target_os = "windows")]
pub mod taskbar_widget;
#[cfg(desktop)]
pub mod tray;
pub mod types;
pub mod web;

#[cfg(target_os = "macos")]
use tauri::Emitter;

use commands::{
    ack_close_behavior_prompt, add_account_from_file, add_api_account, cancel_login,
    check_codex_processes, complete_close_behavior, complete_login, delete_account,
    detect_local_auth_json, export_accounts_full_encrypted_file, export_accounts_slim_text,
    get_account_usage_stats, get_active_account_info, get_api_account_config, get_app_language,
    get_app_settings, get_dock_display_mode, get_masked_account_ids, get_usage,
    import_accounts_full_encrypted_file, import_accounts_slim_text, kill_codex_processes,
    list_accounts, open_main_window, quit_app, refresh_account_metadata,
    refresh_all_accounts_usage, rename_account, report_usage, set_account_disabled,
    set_api_account_config, set_app_language, set_app_settings, set_dock_display_mode,
    set_masked_account_ids, start_login, switch_account, warmup_account, warmup_all_accounts,
};
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();
    #[cfg(desktop)]
    {
        // Must be registered before every other plugin so a second desktop
        // process cannot race the account/config transaction files.
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            commands::restore_main_window(app);
        }));
        builder = builder.plugin(
            tauri_plugin_window_state::Builder::new()
                .with_filter(|label| label == "main")
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED,
                )
                .build(),
        );
    }

    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            {
                let _transition_guard =
                    commands::lock_account_transition().map_err(std::io::Error::other)?;
            }
            #[cfg(desktop)]
            {
                app.handle()
                    .plugin(tauri_plugin_updater::Builder::new().build())?;
                app_menu::setup(app.handle())?;
                tray::setup(app.handle())?;
                floating::setup(app.handle())?;
                #[cfg(target_os = "windows")]
                taskbar_widget::setup(app.handle());

                // The plugin restores on window-ready. Reapply once after the
                // desktop helpers finish initializing so Windows also keeps a
                // restored maximized state through startup-time window setup.
                if let Some(window) = tauri::Manager::get_webview_window(app, "main") {
                    use tauri_plugin_window_state::WindowExt;

                    window.restore_state(
                        tauri_plugin_window_state::StateFlags::SIZE
                            | tauri_plugin_window_state::StateFlags::POSITION
                            | tauri_plugin_window_state::StateFlags::MAXIMIZED,
                    )?;
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            #[cfg(desktop)]
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    #[cfg(target_os = "macos")]
                    if commands::should_prompt_for_close_behavior() {
                        let payload = commands::window::next_close_behavior_prompt_payload();
                        let app_handle = tauri::Manager::app_handle(window);
                        commands::window::schedule_close_behavior_prompt_fallback(
                            app_handle.clone(),
                            payload.request_id,
                        );
                        let _ =
                            window.emit(commands::window::CLOSE_BEHAVIOR_REQUESTED_EVENT, payload);
                        return;
                    }
                    commands::hide_main_window(&tauri::Manager::app_handle(window));
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::open_codex_app,
            // Account management
            list_accounts,
            get_active_account_info,
            detect_local_auth_json,
            add_account_from_file,
            add_api_account,
            switch_account,
            set_api_account_config,
            get_api_account_config,
            delete_account,
            rename_account,
            set_account_disabled,
            export_accounts_slim_text,
            import_accounts_slim_text,
            export_accounts_full_encrypted_file,
            import_accounts_full_encrypted_file,
            // Masked accounts
            get_masked_account_ids,
            set_masked_account_ids,
            // OAuth
            start_login,
            complete_login,
            cancel_login,
            // Usage
            get_usage,
            get_account_usage_stats,
            refresh_account_metadata,
            refresh_all_accounts_usage,
            warmup_account,
            warmup_all_accounts,
            // Process detection
            check_codex_processes,
            kill_codex_processes,
            // Tray integration
            open_main_window,
            quit_app,
            report_usage,
            get_dock_display_mode,
            set_dock_display_mode,
            complete_close_behavior,
            ack_close_behavior_prompt,
            get_app_language,
            set_app_language,
            get_app_settings,
            set_app_settings,
            floating::set_floating_bounds,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, _event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = _event {
                commands::restore_main_window(_app);
            }
        });
}
