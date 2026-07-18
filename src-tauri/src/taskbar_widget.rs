//! Windows-only taskbar usage widget. The Shell hierarchy is not public API,
//! so this module is deliberately isolated and fails closed.

use std::{
    sync::{atomic::{AtomicIsize, AtomicU32, Ordering}, LazyLock, Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};

use tauri::{AppHandle, Runtime};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{COLORREF, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_QUALITY, DeleteDC, DeleteObject, DrawTextW, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_SINGLELINE, DT_VCENTER, EndPaint, FillRect, FW_NORMAL, GetDC, GetPixel, InvalidateRect, PAINTSTRUCT, ReleaseDC, SelectObject, SetBkMode, SetTextColor, SRCCOPY, TRANSPARENT},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::{GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
            WindowsAndMessaging::{CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowExW, FindWindowW, GetClientRect, GetParent, GetWindowLongW, GetWindowRect, IsWindow, LoadCursorW, RegisterClassW, RegisterWindowMessageW, SetLayeredWindowAttributes, SetParent, SetWindowLongW, SetWindowPos, ShowWindow, TranslateMessage, CS_DBLCLKS, GWL_STYLE, HMENU, IDC_ARROW, LWA_COLORKEY, MSG, SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER, WM_DESTROY, WM_ERASEBKGND, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEWHEEL, WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP, WNDCLASSW, WS_CHILD, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE},
        },
    },
};

use crate::{
    auth::{load_accounts, load_app_settings},
    types::{AppSettings, TaskbarDoubleClickAction, TaskbarLayout, UsageInfo},
};

const CLASS_NAME: PCWSTR = w!("CodexSwitcherTaskbarWidget");
const POSITION_REFRESH_INTERVAL: Duration = Duration::from_millis(500);
const COUNTDOWN_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
static HWND_WIDGET: AtomicIsize = AtomicIsize::new(0);
static TASKBAR_CREATED_MESSAGE: AtomicU32 = AtomicU32::new(0);
static LAST_DARK_MODE: AtomicU32 = AtomicU32::new(0);
static BACKGROUND_KEY: AtomicU32 = AtomicU32::new(0x00F3F3F3);
static APP: OnceLock<AppHandle> = OnceLock::new();
static MODEL: LazyLock<Mutex<WidgetModel>> = LazyLock::new(|| Mutex::new(WidgetModel::default()));

#[derive(Default)]
struct WidgetModel {
    account_id: Option<String>,
    primary: Option<f64>,
    secondary: Option<f64>,
    has_primary_window: bool,
    has_secondary_window: bool,
    primary_resets_at: Option<i64>,
    secondary_resets_at: Option<i64>,
    account: String,
    layout: TaskbarLayout,
    enabled: bool,
    chinese: bool,
    offset_x: i32,
    offset_y: i32,
}

pub fn setup(app: &AppHandle) {
    let _ = APP.set(app.clone());
    refresh_model(None);
    if let Err(error) = thread::Builder::new().name("taskbar-widget".into()).spawn(widget_thread) {
        eprintln!("Failed to spawn taskbar widget thread: {error}");
    }
}

pub fn apply_settings<R: Runtime>(_app: &AppHandle<R>, _settings: &AppSettings) {
    refresh_model(None);
    let hwnd = hwnd_widget();
    if !hwnd.0.is_null() { unsafe { position_widget(hwnd); } }
    invalidate();
}

pub fn ingest_usage(usage: &UsageInfo) {
    refresh_model(Some(usage));
    invalidate();
}

pub fn refresh_active_account() {
    refresh_model(None);
    invalidate();
}

fn refresh_model(usage: Option<&UsageInfo>) {
    let settings = load_app_settings().unwrap_or_default();
    let store = load_accounts().unwrap_or_default();
    let active_id = store.active_account_id.as_deref();
    let account = active_id
        .and_then(|id| store.accounts.iter().find(|account| account.id == id));
    if let Ok(mut model) = MODEL.lock() {
        if model.account_id.as_deref() != active_id {
            model.primary = None;
            model.secondary = None;
            model.has_primary_window = false;
            model.has_secondary_window = false;
            model.primary_resets_at = None;
            model.secondary_resets_at = None;
        }
        model.account_id = active_id.map(str::to_owned);
        model.enabled = settings.taskbar.enabled;
        model.layout = settings.taskbar.layout;
        model.offset_x = settings.taskbar.offset_x;
        model.offset_y = settings.taskbar.offset_y;
        let language = settings.language.as_str();
        model.chinese = language.starts_with("zh")
            || (language == crate::types::AppLanguage::SYSTEM_CODE
                && sys_locale::get_locale().is_some_and(|locale| locale.starts_with("zh")));
        model.account = account
            .map(|item| item.name.clone())
            .unwrap_or_else(|| "--".into());
        if let Some(usage) = usage.filter(|item| Some(item.account_id.as_str()) == active_id && item.error.is_none()) {
            model.primary = remaining(usage.primary_used_percent);
            model.secondary = remaining(usage.secondary_used_percent);
            model.has_primary_window = usage.primary_used_percent.is_some()
                || usage.primary_window_minutes.is_some()
                || usage.primary_resets_at.is_some();
            model.has_secondary_window = usage.secondary_used_percent.is_some()
                || usage.secondary_window_minutes.is_some()
                || usage.secondary_resets_at.is_some();
            model.primary_resets_at = usage.primary_resets_at;
            model.secondary_resets_at = usage.secondary_resets_at;
        }
    }
}

fn remaining(used: Option<f64>) -> Option<f64> {
    used.filter(|value| value.is_finite()).map(|value| (100.0 - value).clamp(0.0, 100.0))
}

fn invalidate() {
    let hwnd = hwnd_widget();
    if !hwnd.0.is_null() { unsafe { let _ = InvalidateRect(hwnd, None, false); } }
}

fn hwnd_widget() -> HWND {
    HWND(HWND_WIDGET.load(Ordering::Relaxed) as *mut core::ffi::c_void)
}

fn widget_thread() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let module = match GetModuleHandleW(None) { Ok(value) => value, Err(error) => { eprintln!("Taskbar widget: GetModuleHandleW failed: {error}"); return; } };
        let instance: HINSTANCE = module.into();
        let class = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 && GetLastError().0 != 1410 {
            eprintln!("Taskbar widget: RegisterClassW failed: {}", GetLastError().0);
            update_last_error(Some("Could not register the native taskbar widget window."));
            return;
        }
        TASKBAR_CREATED_MESSAGE.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);

        let mut last_attach = Instant::now() - Duration::from_secs(5);
        let mut last_countdown_refresh = Instant::now();
        let mut attach_failures = 0u8;
        loop {
            let current = hwnd_widget();
            let enabled = MODEL.lock().map(|model| model.enabled).unwrap_or(false);
            if enabled && (!IsWindow(current).as_bool() || current.0.is_null()) && last_attach.elapsed() >= POSITION_REFRESH_INTERVAL {
                last_attach = Instant::now();
                if attach(instance) {
                    attach_failures = 0;
                    update_last_error(None);
                } else {
                    attach_failures = attach_failures.saturating_add(1);
                    if attach_failures >= 3 {
                        update_last_error(Some("Could not attach to the Windows taskbar. Only a bottom primary taskbar is supported."));
                    }
                }
            } else if !current.0.is_null() && last_attach.elapsed() >= POSITION_REFRESH_INTERVAL {
                last_attach = Instant::now();
                position_widget(current);
            }

            let mut msg = MSG::default();
            while windows::Win32::UI::WindowsAndMessaging::PeekMessageW(&mut msg, None, 0, 0, windows::Win32::UI::WindowsAndMessaging::PM_REMOVE).as_bool() {
                if msg.message == windows::Win32::UI::WindowsAndMessaging::WM_QUIT { return; }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if last_countdown_refresh.elapsed() >= COUNTDOWN_REFRESH_INTERVAL {
                last_countdown_refresh = Instant::now();
                if !current.0.is_null() {
                    let _ = InvalidateRect(current, None, false);
                }
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}

unsafe fn attach(instance: windows::Win32::Foundation::HINSTANCE) -> bool {
    let taskbar = FindWindowW(w!("Shell_TrayWnd"), None).unwrap_or_default();
    if taskbar.0.is_null() { eprintln!("Taskbar widget: Shell_TrayWnd not found"); return false; }
    let notify = FindWindowExW(taskbar, None, w!("TrayNotifyWnd"), None).unwrap_or_default();
    if notify.0.is_null() { eprintln!("Taskbar widget: TrayNotifyWnd not found"); return false; }

    let mut taskbar_rect = RECT::default();
    let mut notify_rect = RECT::default();
    if GetWindowRect(taskbar, &mut taskbar_rect).is_err() || GetWindowRect(notify, &mut notify_rect).is_err() { return false; }
    if taskbar_rect.bottom < notify_rect.bottom || taskbar_rect.bottom - taskbar_rect.top > 100 { return false; }

    let dpi = GetDpiForWindow(taskbar).max(96) as i32;
    let width = 224 * dpi / 96;
    let height = taskbar_rect.bottom - taskbar_rect.top;
    let anchor_left = left_edge_before_notify(taskbar_rect, notify_rect);
    let (offset_x, offset_y) = MODEL.lock().map(|model| (model.offset_x, model.offset_y)).unwrap_or_default();
    let x = anchor_left - width - (12 * dpi / 96) + offset_x;
    if x < taskbar_rect.left { return false; }

    let background = sample_taskbar_background(x - 1, taskbar_rect.top + height / 2);
    BACKGROUND_KEY.store(background.0, Ordering::Relaxed);
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED, CLASS_NAME, w!("Codex Usage"),
        WS_POPUP | WS_VISIBLE,
        x, taskbar_rect.top + offset_y, width, height, None, HMENU(core::ptr::null_mut()), instance, None,
    ).unwrap_or_default();
    if hwnd.0.is_null() { eprintln!("Taskbar widget: CreateWindowExW failed: {}", GetLastError().0); return false; }
    let set_parent_result = SetParent(hwnd, taskbar);
    let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
    let _ = SetWindowLongW(hwnd, GWL_STYLE, ((style & !WS_POPUP.0) | WS_CHILD.0) as i32);
    let actual_parent = GetParent(hwnd).unwrap_or_default();
    if actual_parent != taskbar {
        eprintln!("Taskbar widget: SetParent failed: result={set_parent_result:?}, error={}", GetLastError().0);
        let _ = DestroyWindow(hwnd);
        return false;
    }
    if SetLayeredWindowAttributes(hwnd, background, 255, LWA_COLORKEY).is_err() {
        eprintln!("Taskbar widget: failed to set color key: {}", GetLastError().0);
        let _ = DestroyWindow(hwnd);
        return false;
    }
    let dark = is_dark_mode();
    LAST_DARK_MODE.store(if dark { 1 } else { 2 }, Ordering::Relaxed);
    HWND_WIDGET.store(hwnd.0 as isize, Ordering::Relaxed);
    position_widget(hwnd);
    let _ = InvalidateRect(hwnd, None, false);
    true
}

fn update_last_error(error: Option<&str>) {
    let mut settings = load_app_settings().unwrap_or_default();
    let next = error.map(str::to_string);
    if settings.taskbar.last_error == next { return; }
    settings.taskbar.last_error = next;
    if crate::auth::save_app_settings(&settings).is_ok() {
        if let Some(app) = APP.get() {
            use tauri::Emitter;
            let _ = app.emit(crate::commands::settings::SETTINGS_CHANGED_EVENT, settings);
        }
    }
}

unsafe fn position_widget(hwnd: HWND) {
    let taskbar = FindWindowW(w!("Shell_TrayWnd"), None).unwrap_or_default();
    let notify = FindWindowExW(taskbar, None, w!("TrayNotifyWnd"), None).unwrap_or_default();
    if taskbar.0.is_null() || notify.0.is_null() { let _ = DestroyWindow(hwnd); return; }
    let mut taskbar_rect = RECT::default();
    let mut notify_rect = RECT::default();
    let _ = GetWindowRect(taskbar, &mut taskbar_rect);
    let _ = GetWindowRect(notify, &mut notify_rect);
    let dpi = GetDpiForWindow(taskbar).max(96) as i32;
    let width = 224 * dpi / 96;
    let anchor_left = left_edge_before_notify(taskbar_rect, notify_rect);
    let (enabled, offset_x, offset_y) = MODEL.lock().map(|model| (model.enabled, model.offset_x, model.offset_y)).unwrap_or((false, 0, 0));
    let x = anchor_left - width - (12 * dpi / 96) + offset_x;
    let y = taskbar_rect.top + offset_y;
    let height = taskbar_rect.bottom - taskbar_rect.top;
    let mut current_rect = RECT::default();
    let _ = GetWindowRect(hwnd, &mut current_rect);
    if current_rect.left != x || current_rect.top != y || current_rect.right - current_rect.left != width || current_rect.bottom - current_rect.top != height {
        let _ = SetWindowPos(hwnd, None, x - taskbar_rect.left, offset_y, width, height, SWP_NOACTIVATE | SWP_NOZORDER);
    }
    let background = sample_taskbar_background(x - 1, taskbar_rect.top + height / 2);
    if BACKGROUND_KEY.swap(background.0, Ordering::Relaxed) != background.0 {
        let _ = SetLayeredWindowAttributes(hwnd, background, 255, LWA_COLORKEY);
        let _ = InvalidateRect(hwnd, None, false);
    }
    let dark = is_dark_mode();
    let theme = if dark { 1 } else { 2 };
    if LAST_DARK_MODE.swap(theme, Ordering::Relaxed) != theme {
        let _ = InvalidateRect(hwnd, None, false);
    }
    let _ = ShowWindow(hwnd, if enabled { SW_SHOW } else { SW_HIDE });
}

unsafe fn left_edge_before_notify(taskbar: RECT, notify: RECT) -> i32 {
    let traffic = FindWindowW(None, w!("TrafficMonitorTaskbarWindow")).unwrap_or_default();
    if !traffic.0.is_null() {
        let mut rect = RECT::default();
        if GetWindowRect(traffic, &mut rect).is_ok()
            && rect.left < notify.left
            && rect.right > taskbar.left
            && rect.top < taskbar.bottom
            && rect.bottom > taskbar.top
        {
            return rect.left;
        }
    }
    notify.left
}

unsafe fn sample_taskbar_background(x: i32, y: i32) -> COLORREF {
    let screen_dc = GetDC(None);
    if screen_dc.0.is_null() {
        return fallback_background();
    }
    let color = GetPixel(screen_dc, x.max(0), y.max(0));
    let _ = ReleaseDC(None, screen_dc);
    if color.0 == u32::MAX { fallback_background() } else { color }
}

fn fallback_background() -> COLORREF {
    if is_dark_mode() { COLORREF(0x00202020) } else { COLORREF(0x00F3F3F3) }
}

extern "system" fn wnd_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if message == TASKBAR_CREATED_MESSAGE.load(Ordering::Relaxed) {
            position_widget(hwnd);
            return LRESULT(0);
        }
        match message {
            WM_PAINT => { paint(hwnd); LRESULT(0) }
            WM_ERASEBKGND => LRESULT(1),
            WM_LBUTTONDBLCLK => { handle_double_click(); position_widget(hwnd); LRESULT(0) }
            WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MOUSEWHEEL => {
                LRESULT(0)
            }
            WM_DESTROY => { HWND_WIDGET.store(0, Ordering::Relaxed); LRESULT(0) }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

unsafe fn paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    render_gdi(hwnd, hdc);
    let _ = EndPaint(hwnd, &ps);
}

unsafe fn render_gdi(hwnd: HWND, hdc: windows::Win32::Graphics::Gdi::HDC) {
    let mut rect = RECT::default();
    let _ = GetClientRect(hwnd, &mut rect);
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 0 || height <= 0 { return; }

    let memory_dc = CreateCompatibleDC(hdc);
    if memory_dc.0.is_null() { return; }
    let bitmap = CreateCompatibleBitmap(hdc, width, height);
    if bitmap.0.is_null() { let _ = DeleteDC(memory_dc); return; }
    let old_bitmap = SelectObject(memory_dc, bitmap);
    let background = COLORREF(BACKGROUND_KEY.load(Ordering::Relaxed));
    let brush = CreateSolidBrush(background);
    let _ = FillRect(memory_dc, &rect, brush);
    let _ = DeleteObject(brush);

    let dark = is_dark_mode();
    let foreground = if dark { 0x00F2F2F2 } else { 0x001A1A1A };
    let dpi = GetDpiForWindow(hwnd).max(96) as i32;
    let font = CreateFontW(
        -(9 * dpi / 72), 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0,
        DEFAULT_CHARSET.0 as u32, 0, 0, DEFAULT_QUALITY.0 as u32, 0, w!("Microsoft YaHei"),
    );
    let old_font = SelectObject(memory_dc, font);
    let _ = SetBkMode(memory_dc, TRANSPARENT);
    let _ = SetTextColor(memory_dc, COLORREF(foreground));

    let (layout, line1, line2, weekly_only) = formatted_lines();
    if layout == TaskbarLayout::Detailed && weekly_only {
        let [weekly, reset, _, account] = formatted_detailed_cells();
        let row_height = 16 * dpi / 96;
        let content_height = row_height * 2;
        let content_top = ((rect.bottom - rect.top - content_height) / 2).max(0);
        let first_left = 16 * dpi / 96;
        let second_left = 88 * dpi / 96;
        let column_gap = 4 * dpi / 96;
        let right = rect.right - 4 * dpi / 96;
        let mut weekly_rect = RECT { left: first_left, top: content_top, right: second_left - column_gap, bottom: content_top + content_height };
        let mut reset_rect = RECT { left: second_left, top: content_top, right, bottom: content_top + row_height };
        let mut account_rect = RECT { left: second_left, top: content_top + row_height, right, bottom: content_top + content_height };
        draw_left(&mut weekly_rect, &weekly, memory_dc);
        draw_left(&mut reset_rect, &reset, memory_dc);
        draw_left(&mut account_rect, &account, memory_dc);
    } else if layout == TaskbarLayout::Detailed {
        let [top_left, top_right, bottom_left, bottom_right] = formatted_detailed_cells();
        let row_height = 16 * dpi / 96;
        let content_height = row_height * 2;
        let content_top = ((rect.bottom - rect.top - content_height) / 2).max(0);
        let first_left = 16 * dpi / 96;
        let second_left = 88 * dpi / 96;
        let column_gap = 4 * dpi / 96;
        let right = rect.right - 4 * dpi / 96;
        let mut top_first = RECT { left: first_left, top: content_top, right: second_left - column_gap, bottom: content_top + row_height };
        let mut top_second = RECT { left: second_left, top: content_top, right, bottom: content_top + row_height };
        let mut bottom_first = RECT { left: first_left, top: content_top + row_height, right: second_left - column_gap, bottom: content_top + content_height };
        let mut bottom_second = RECT { left: second_left, top: content_top + row_height, right, bottom: content_top + content_height };
        draw_left(&mut top_first, &top_left, memory_dc);
        draw_left(&mut top_second, &top_right, memory_dc);
        draw_left(&mut bottom_first, &bottom_left, memory_dc);
        draw_left(&mut bottom_second, &bottom_right, memory_dc);
    } else if layout == TaskbarLayout::Compact {
        let mut line = rect;
        draw(&mut line, &line1, memory_dc);
    } else {
        // TrafficMonitor renders its two rows as one compact block rather than
        // splitting the full taskbar height into two large halves.
        let row_height = 16 * dpi / 96;
        let content_height = row_height * 2;
        let content_top = ((rect.bottom - rect.top - content_height) / 2).max(0);
        let mut top = RECT { top: content_top, bottom: content_top + row_height, ..rect };
        let mut bottom = RECT { top: content_top + row_height, bottom: content_top + content_height, ..rect };
        draw(&mut top, &line1, memory_dc);
        draw(&mut bottom, &line2, memory_dc);
    }
    let _ = SelectObject(memory_dc, old_font);
    let _ = DeleteObject(font);
    let _ = BitBlt(hdc, 0, 0, width, height, memory_dc, 0, 0, SRCCOPY);

    let _ = SelectObject(memory_dc, old_bitmap);
    let _ = DeleteObject(bitmap);
    let _ = DeleteDC(memory_dc);
}

unsafe fn draw(rect: &mut RECT, value: &str, hdc: windows::Win32::Graphics::Gdi::HDC) {
    if value.is_empty() { return; }
    let mut wide: Vec<u16> = value.encode_utf16().collect();
    let _ = DrawTextW(hdc, &mut wide, rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
}

unsafe fn draw_left(rect: &mut RECT, value: &str, hdc: windows::Win32::Graphics::Gdi::HDC) {
    if value.is_empty() { return; }
    let mut wide: Vec<u16> = value.encode_utf16().collect();
    let _ = DrawTextW(hdc, &mut wide, rect, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
}

fn formatted_detailed_cells() -> [String; 4] {
    let model = MODEL.lock().unwrap_or_else(|error| error.into_inner());
    let primary = model.primary.map(|value| format!("{value:.0}%")).unwrap_or_else(|| "--".into());
    let secondary = model.secondary.map(|value| format!("{value:.0}%")).unwrap_or_else(|| "--".into());
    let weekly_only = !model.has_primary_window && model.has_secondary_window;
    let reset = reset_label(
        if weekly_only { model.secondary_resets_at } else { model.primary_resets_at },
        model.chinese,
    );
    if weekly_only {
        return if model.chinese {
            [
                format!("周: {secondary}"),
                format!("重置: {reset}"),
                String::new(),
                format!("账号: {}", model.account),
            ]
        } else {
            [
                format!("Week: {secondary}"),
                format!("Reset: {reset}"),
                String::new(),
                model.account.clone(),
            ]
        };
    }

    if model.chinese {
        [
            format!("5H: {primary}"),
            format!("重置: {reset}"),
            format!("周: {secondary}"),
            format!("账号: {}", model.account),
        ]
    } else {
        [
            format!("5H: {primary}"),
            format!("Reset: {reset}"),
            format!("Week: {secondary}"),
            model.account.clone(),
        ]
    }
}

fn formatted_lines() -> (TaskbarLayout, String, String, bool) {
    let model = MODEL.lock().unwrap_or_else(|error| error.into_inner());
    let p = model.primary.map(|v| format!("{v:.0}%")).unwrap_or_else(|| "--".into());
    let s = model.secondary.map(|v| format!("{v:.0}%")).unwrap_or_else(|| "--".into());
    let weekly_only = !model.has_primary_window && model.has_secondary_window;
    let reset = reset_label(
        if weekly_only { model.secondary_resets_at } else { model.primary_resets_at },
        model.chinese,
    );

    if weekly_only {
        return match model.layout {
            TaskbarLayout::Detailed if model.chinese => (model.layout, format!("周 {s}"), format!("重置 {reset}"), true),
            TaskbarLayout::Detailed => (model.layout, format!("Week {s}"), format!("Reset {reset}"), true),
            TaskbarLayout::Minimal if model.chinese => (model.layout, format!("周：{s}"), format!("重置：{reset}"), false),
            TaskbarLayout::Minimal => (model.layout, format!("Week: {s}"), format!("Reset: {reset}"), false),
            TaskbarLayout::Compact if model.chinese => (model.layout, format!("周 {s}  ·  {reset}"), String::new(), false),
            TaskbarLayout::Compact => (model.layout, format!("W {s}  ·  {reset}"), String::new(), false),
        };
    }

    match model.layout {
        TaskbarLayout::Detailed if model.chinese => (model.layout, format!("5H：{p}  重置：{reset}"), format!("周：{s}  账号：{}", model.account), false),
        TaskbarLayout::Detailed => (model.layout, format!("5H: {p}  Reset: {reset}"), format!("Week: {s}  {}", model.account), false),
        TaskbarLayout::Minimal if model.chinese => (model.layout, format!("5H：{p} · {reset}"), format!("周：{s}"), false),
        TaskbarLayout::Minimal => (model.layout, format!("5H: {p} · {reset}"), format!("Week: {s}"), false),
        TaskbarLayout::Compact if model.chinese => (model.layout, format!("5H {p} · 周 {s} · {reset}"), String::new(), false),
        TaskbarLayout::Compact => (model.layout, format!("5H {p} · W {s} · {reset}"), String::new(), false),
    }
}

fn reset_label(timestamp: Option<i64>, chinese: bool) -> String {
    reset_label_at(timestamp, chrono::Utc::now().timestamp(), chinese)
}

fn reset_label_at(timestamp: Option<i64>, now: i64, chinese: bool) -> String {
    let Some(timestamp) = timestamp else { return "--".into(); };
    let remaining_seconds = timestamp - now;
    if remaining_seconds < 60 {
        return if chinese { "现在".into() } else { "Now".into() };
    }

    if remaining_seconds >= 24 * 60 * 60 {
        let days = remaining_seconds / (24 * 60 * 60);
        if chinese { format!("{days}天") } else { format!("{days}d") }
    } else if remaining_seconds >= 60 * 60 {
        let hours = remaining_seconds / (60 * 60);
        if chinese { format!("{hours}小时") } else { format!("{hours}h") }
    } else {
        let minutes = remaining_seconds / 60;
        if chinese { format!("{minutes}分钟") } else { format!("{minutes}m") }
    }
}

fn is_dark_mode() -> bool {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
        .ok()
        .and_then(|key| key.get_value::<u32, _>("SystemUsesLightTheme").ok())
        .map(|value| value == 0)
        .unwrap_or(true)
}

fn handle_double_click() {
    let Some(app) = APP.get() else { return; };
    let action = load_app_settings().unwrap_or_default().taskbar.double_click_action;
    match action {
        TaskbarDoubleClickAction::ToggleFloating => crate::floating::toggle(app),
        TaskbarDoubleClickAction::OpenMain => crate::commands::restore_main_window(app),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_is_clamped() {
        assert_eq!(remaining(Some(-5.0)), Some(100.0));
        assert_eq!(remaining(Some(110.0)), Some(0.0));
        assert_eq!(remaining(Some(f64::NAN)), None);
    }

    #[test]
    fn weekly_only_usage_hides_session_and_shows_weekly_reset() {
        {
            let mut model = MODEL.lock().unwrap_or_else(|error| error.into_inner());
            model.layout = TaskbarLayout::Detailed;
            model.chinese = false;
            model.primary = None;
            model.secondary = Some(65.0);
            model.has_primary_window = false;
            model.has_secondary_window = true;
            model.primary_resets_at = None;
            model.secondary_resets_at = Some(chrono::Utc::now().timestamp() + 3 * 24 * 60 * 60 + 60 * 60);
            model.account = "work".into();
        }

        let (_, _, _, weekly_only) = formatted_lines();
        assert!(weekly_only);

        let cells = formatted_detailed_cells();
        assert_eq!(cells, ["Week: 65%", "Reset: 3d", "", "work"]);
        assert!(cells.iter().all(|cell| !cell.contains("5H")));
    }

    #[test]
    fn reset_labels_use_adaptive_localized_units() {
        let now = 1_800_000_000;
        assert_eq!(reset_label_at(Some(now + 3 * 24 * 60 * 60 + 4 * 60 * 60), now, true), "3天");
        assert_eq!(reset_label_at(Some(now + 4 * 60 * 60 + 27 * 60), now, true), "4小时");
        assert_eq!(reset_label_at(Some(now + 27 * 60), now, true), "27分钟");
        assert_eq!(reset_label_at(Some(now), now, true), "现在");
        assert_eq!(reset_label_at(Some(now + 3 * 24 * 60 * 60), now, false), "3d");
        assert_eq!(reset_label_at(Some(now + 4 * 60 * 60), now, false), "4h");
        assert_eq!(reset_label_at(Some(now + 27 * 60), now, false), "27m");
        assert_eq!(reset_label_at(Some(now), now, false), "Now");
    }

    #[test]
    fn reset_labels_do_not_roll_up_before_unit_boundaries() {
        let now = 1_800_000_000;
        assert_eq!(reset_label_at(Some(now + 24 * 60 * 60 - 1), now, true), "23小时");
        assert_eq!(reset_label_at(Some(now + 60 * 60 - 1), now, true), "59分钟");
        assert_eq!(reset_label_at(Some(now + 59), now, true), "现在");
    }
}
