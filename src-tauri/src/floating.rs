use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, Runtime, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use crate::{auth::{load_app_settings, save_app_settings}, types::AppSettings};

pub const FLOATING_WINDOW: &str = "floating";
pub const FLOATING_CONTROLS_WINDOW: &str = "floating-controls";
pub const FLOATING_SETTINGS_EVENT: &str = "floating-settings-changed";
const WIDTH: f64 = 300.0;
const HEIGHT: f64 = 184.0;
const MIN_WIDTH: f64 = 180.0;
const MIN_HEIGHT: f64 = 110.0;
const COMPACT_SIZE: f64 = 48.0;

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    if app.get_webview_window(FLOATING_WINDOW).is_none() {
        let mut settings = load_app_settings().unwrap_or_default();
        if settings.floating.normalize_modes(None) {
            let _ = save_app_settings(&settings);
        }
        let saved_size = settings.floating.size.unwrap_or((WIDTH as u32, HEIGHT as u32));
        let initial_size = if settings.floating.compact_mode {
            (COMPACT_SIZE, COMPACT_SIZE)
        } else {
            (saved_size.0 as f64, saved_size.1 as f64)
        };
        let minimum_size = if settings.floating.compact_mode {
            (COMPACT_SIZE, COMPACT_SIZE)
        } else {
            (MIN_WIDTH, MIN_HEIGHT)
        };
        let builder = WebviewWindowBuilder::new(
            app,
            FLOATING_WINDOW,
            WebviewUrl::App("floating.html".into()),
        )
        .title("Codex Usage")
        .inner_size(initial_size.0, initial_size.1)
        .min_inner_size(minimum_size.0, minimum_size.1)
        .resizable(true)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(settings.floating.always_on_top)
        .skip_taskbar(true)
        .visible(settings.floating.enabled && settings.floating.visible);

        let window = builder.build()?;
        let _ = window.remove_menu();
        if let Some((x, y)) = settings.floating.position {
            // `outer_position` reports physical pixels. Restore with the same
            // coordinate type so Windows display scaling does not shift the
            // widget between sessions.
            let _ = window.set_position(PhysicalPosition::new(x, y));
        }
        let _ = window.set_ignore_cursor_events(settings.floating.click_through);
        create_controls_window(app, &settings)?;
        if let Ok(position) = window.outer_position() {
            position_controls(app, position);
        }
        let app_handle = app.clone();
        window.on_window_event(move |event| {
            match event {
                WindowEvent::Moved(position) => {
                    let mut settings = load_app_settings().unwrap_or_default();
                    let next = Some((position.x, position.y));
                    // Hover preview temporarily enlarges and repositions the native host.
                    // Keep the persisted coordinate tied to the visible 48x48 compact card;
                    // the final move after shrinking back to compact is saved normally.
                    let expanded_compact_preview = settings.floating.compact_mode
                        && app_handle
                            .get_webview_window(FLOATING_WINDOW)
                            .and_then(|window| {
                                let size = window.inner_size().ok()?;
                                let scale = window.scale_factor().ok()?;
                                Some(
                                    size.width as f64 / scale > COMPACT_SIZE + 2.0
                                        || size.height as f64 / scale > COMPACT_SIZE + 2.0,
                                )
                            })
                            .unwrap_or(false);
                    if !expanded_compact_preview && settings.floating.position != next {
                        settings.floating.position = next;
                        let _ = save_app_settings(&settings);
                        let _ = app_handle.emit(crate::commands::settings::SETTINGS_CHANGED_EVENT, settings);
                    }
                    position_controls(&app_handle, *position);
                }
                WindowEvent::Resized(size) => {
                    let mut settings = load_app_settings().unwrap_or_default();
                    let compact_mode = settings.floating.compact_mode;
                    if !compact_mode {
                        let scale = app_handle
                            .get_webview_window(FLOATING_WINDOW)
                            .and_then(|window| window.scale_factor().ok())
                            .unwrap_or(1.0);
                        let next = Some((
                            (size.width as f64 / scale).round() as u32,
                            (size.height as f64 / scale).round() as u32,
                        ));
                        if settings.floating.size != next {
                            settings.floating.size = next;
                            let _ = save_app_settings(&settings);
                            let _ = app_handle.emit(
                                crate::commands::settings::SETTINGS_CHANGED_EVENT,
                                settings,
                            );
                        }
                    }
                    if let Some(main) = app_handle.get_webview_window(FLOATING_WINDOW) {
                        if let Ok(position) = main.outer_position() {
                            position_controls(&app_handle, position);
                        }
                    }
                }
                _ => {}
            }
        });
    }
    Ok(())
}

fn create_controls_window<R: Runtime>(app: &AppHandle<R>, settings: &AppSettings) -> tauri::Result<()> {
    if app.get_webview_window(FLOATING_CONTROLS_WINDOW).is_some() { return Ok(()); }
    let position = settings.floating.position.unwrap_or((100, 100));
    let window = WebviewWindowBuilder::new(
        app,
        FLOATING_CONTROLS_WINDOW,
        WebviewUrl::App("floating-controls.html".into()),
    )
    .title("Codex Usage Controls")
    .inner_size(180.0, 62.0)
    .position((position.0 + 216) as f64, (position.1 + 20) as f64)
    .resizable(false)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(
        settings.floating.enabled
            && settings.floating.visible
            && settings.floating.click_through,
    )
    .build()?;
    let _ = window.remove_menu();
    Ok(())
}

fn position_controls<R: Runtime>(app: &AppHandle<R>, position: PhysicalPosition<i32>) {
    let Some(main) = app.get_webview_window(FLOATING_WINDOW) else { return; };
    let Some(controls) = app.get_webview_window(FLOATING_CONTROLS_WINDOW) else { return; };
    let Ok(main_size) = main.outer_size() else { return; };
    let Ok(control_size) = controls.outer_size() else { return; };
    let scale = main.scale_factor().unwrap_or(1.0);
    // The full floating card now fills its native window. Match the in-card
    // controls (`right-4 top-3`) so enabling click-through does not shift them.
    let right_margin = (16.0 * scale).round() as i32;
    let top_offset = (12.0 * scale).round() as i32;
    let x = position.x + main_size.width as i32 - control_size.width as i32 - right_margin;
    let mut target = PhysicalPosition::new(x, position.y + top_offset);

    if let Ok(Some(monitor)) = main.current_monitor() {
        let work = monitor.work_area();
        let max_x = (work.position.x + work.size.width as i32 - control_size.width as i32)
            .max(work.position.x);
        let max_y = (work.position.y + work.size.height as i32 - control_size.height as i32)
            .max(work.position.y);
        target.x = target.x.clamp(work.position.x, max_x);
        target.y = target.y.clamp(work.position.y, max_y);
    }

    place_controls_above_main(&controls, target);
}

pub fn apply_settings<R: Runtime>(app: &AppHandle<R>, settings: &AppSettings) {
    let Some(window) = app.get_webview_window(FLOATING_WINDOW) else { return; };
    let preserved_position = window.outer_position().ok();
    let _ = window.set_ignore_cursor_events(settings.floating.click_through);
    if settings.floating.enabled && settings.floating.visible {
        let _ = window.show();
    } else {
        let _ = window.hide();
    }
    if let Some(position) = preserved_position {
        apply_window_level(
            &window,
            position,
            settings.floating.always_on_top,
            settings.floating.click_through,
        );
    } else {
        let _ = window.set_always_on_top(settings.floating.always_on_top);
        if !settings.floating.always_on_top && !settings.floating.click_through {
            let _ = window.set_focus();
        }
    }
    if let Some(controls) = app.get_webview_window(FLOATING_CONTROLS_WINDOW) {
        if settings.floating.enabled
            && settings.floating.visible
            && settings.floating.click_through
        {
            let _ = controls.show();
            if let Some(position) = preserved_position {
                position_controls(app, position);
            }
        } else {
            let _ = controls.hide();
        }
    }
    let _ = app.emit_to(FLOATING_WINDOW, FLOATING_SETTINGS_EVENT, settings.clone());
}

#[cfg(target_os = "windows")]
fn place_controls_above_main<R: Runtime>(
    controls: &tauri::WebviewWindow<R>,
    position: PhysicalPosition<i32>,
) {
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOSIZE},
    };
    if let Ok(raw) = controls.hwnd() {
        let hwnd = HWND(raw.0 as *mut core::ffi::c_void);
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                position.x,
                position.y,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn place_controls_above_main<R: Runtime>(
    controls: &tauri::WebviewWindow<R>,
    position: PhysicalPosition<i32>,
) {
    let _ = controls.set_always_on_top(true);
    let _ = controls.set_position(position);
}

#[cfg(target_os = "windows")]
fn apply_window_level<R: Runtime>(
    window: &tauri::WebviewWindow<R>,
    position: PhysicalPosition<i32>,
    always_on_top: bool,
    click_through: bool,
) {
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{
            SetWindowPos, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOSIZE,
        },
    };
    if let Ok(raw) = window.hwnd() {
        let hwnd = HWND(raw.0 as *mut core::ffi::c_void);
        let insert_after = if always_on_top { HWND_TOPMOST } else { HWND_NOTOPMOST };
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                insert_after,
                position.x,
                position.y,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        if !always_on_top && !click_through {
            // HWND_NOTOPMOST moves the window to the top of the normal window
            // band; focusing it keeps an unpinned widget in the user's current
            // working layer instead of restoring an old z-order position.
            let _ = window.set_focus();
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn apply_window_level<R: Runtime>(
    window: &tauri::WebviewWindow<R>,
    position: PhysicalPosition<i32>,
    always_on_top: bool,
    click_through: bool,
) {
    let _ = window.set_always_on_top(always_on_top);
    let _ = window.set_position(position);
    if !always_on_top && !click_through {
        let _ = window.set_focus();
    }
}

pub fn toggle<R: Runtime>(app: &AppHandle<R>) {
    let mut settings = load_app_settings().unwrap_or_default();
    settings.floating.enabled = true;
    settings.floating.visible = !settings.floating.visible;
    let _ = save_app_settings(&settings);
    apply_settings(app, &settings);
    let _ = app.emit(crate::commands::settings::SETTINGS_CHANGED_EVENT, settings);
}

pub fn set_click_through<R: Runtime>(app: &AppHandle<R>, enabled: bool) {
    let mut settings = load_app_settings().unwrap_or_default();
    if enabled && settings.floating.compact_mode {
        if let Some(window) = app.get_webview_window(FLOATING_WINDOW) {
            if let Ok(position) = window.outer_position() {
                settings.floating.position = Some((position.x, position.y));
            }
        }
    }
    settings.floating.click_through = enabled;
    if enabled {
        settings.floating.compact_mode = false;
    }
    let _ = save_app_settings(&settings);
    apply_settings(app, &settings);
    let _ = app.emit(crate::commands::settings::SETTINGS_CHANGED_EVENT, settings);
}

#[tauri::command]
pub fn set_floating_bounds(
    app: AppHandle,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<(), String> {
    let window = app
        .get_webview_window(FLOATING_WINDOW)
        .ok_or_else(|| "Floating window is not available".to_string())?;

    #[cfg(target_os = "windows")]
    {
        use windows::Win32::{
            Foundation::HWND,
            UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER},
        };

        let raw = window.hwnd().map_err(|error| error.to_string())?;
        let hwnd = HWND(raw.0 as *mut core::ffi::c_void);
        unsafe {
            SetWindowPos(
                hwnd,
                HWND(core::ptr::null_mut()),
                x,
                y,
                width.max(1) as i32,
                height.max(1) as i32,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )
            .map_err(|error| error.to_string())?;
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        window
            .set_size(tauri::PhysicalSize::new(width.max(1), height.max(1)))
            .map_err(|error| error.to_string())?;
        window
            .set_position(PhysicalPosition::new(x, y))
            .map_err(|error| error.to_string())?;
    }

    Ok(())
}
