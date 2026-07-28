//! Windows-only taskbar usage widget. The Shell hierarchy is not public API,
//! so this module is deliberately isolated and fails closed.

use std::{
    mem,
    sync::{
        atomic::{AtomicIsize, AtomicU32, Ordering},
        LazyLock, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

use tauri::{AppHandle, Runtime};
use windows::{
    core::{w, PCWSTR, PWSTR},
    Win32::{
        Foundation::{
            GetLastError, COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM,
        },
        Graphics::Gdi::{
            BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW,
            CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, GetDC,
            GetPixel, GetTextExtentPoint32W, InvalidateRect, ReleaseDC, SelectObject, SetBkMode,
            SetTextColor, DEFAULT_CHARSET, DEFAULT_QUALITY, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT,
            DT_SINGLELINE, DT_VCENTER, FW_NORMAL, PAINTSTRUCT, SRCCOPY, TRANSPARENT,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::{
                TOOLTIPS_CLASS, TTF_IDISHWND, TTF_SUBCLASS, TTM_ADDTOOLW, TTM_SETMAXTIPWIDTH,
                TTM_UPDATETIPTEXTW, TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW,
            },
            HiDpi::{
                GetDpiForWindow, SetProcessDpiAwarenessContext,
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowExW,
                FindWindowW, GetClientRect, GetParent, GetWindowLongW, GetWindowRect, IsWindow,
                LoadCursorW, RegisterClassW, RegisterWindowMessageW, SendMessageW,
                SetLayeredWindowAttributes, SetParent, SetWindowLongW, SetWindowPos, ShowWindow,
                TranslateMessage, CS_DBLCLKS, CW_USEDEFAULT, GWL_STYLE, HMENU, IDC_ARROW,
                LWA_COLORKEY, MSG, SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_SHOW, WM_DESTROY,
                WM_ERASEBKGND, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEWHEEL,
                WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP, WNDCLASSW, WS_CHILD, WS_EX_LAYERED,
                WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
            },
        },
    },
};

use crate::{
    auth::{load_accounts, load_app_settings},
    types::{
        AppSettings, TaskbarDoubleClickAction, TaskbarLayout, UsageInfo, TASKBAR_MAX_WIDTH,
        TASKBAR_MIN_WIDTH,
    },
};

const CLASS_NAME: PCWSTR = w!("CodexSwitcherTaskbarWidget");
const POSITION_REFRESH_INTERVAL: Duration = Duration::from_millis(500);
const COUNTDOWN_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const MONTHLY_WINDOW_MINUTES_THRESHOLD: i64 = 28 * 24 * 60;
const ACCOUNT_SAFE_MARGIN_PX: i32 = 8;
static HWND_WIDGET: AtomicIsize = AtomicIsize::new(0);
static HWND_TOOLTIP: AtomicIsize = AtomicIsize::new(0);
static TASKBAR_CREATED_MESSAGE: AtomicU32 = AtomicU32::new(0);
static LAST_DARK_MODE: AtomicU32 = AtomicU32::new(0);
static BACKGROUND_KEY: AtomicU32 = AtomicU32::new(0x00F3F3F3);
static APP: OnceLock<AppHandle> = OnceLock::new();
static MODEL: LazyLock<Mutex<WidgetModel>> = LazyLock::new(|| Mutex::new(WidgetModel::default()));
static TOOLTIP_TEXT: LazyLock<Mutex<Vec<u16>>> = LazyLock::new(|| Mutex::new(Vec::new()));

#[derive(Default)]
struct WidgetModel {
    account_id: Option<String>,
    primary: Option<f64>,
    secondary: Option<f64>,
    has_primary_window: bool,
    has_secondary_window: bool,
    primary_window_minutes: Option<i64>,
    primary_resets_at: Option<i64>,
    secondary_resets_at: Option<i64>,
    account: String,
    layout: TaskbarLayout,
    enabled: bool,
    chinese: bool,
    width: i32,
    offset_x: i32,
    offset_y: i32,
}

pub fn setup(app: &AppHandle) {
    let _ = APP.set(app.clone());
    refresh_model(None);
    if let Err(error) = thread::Builder::new()
        .name("taskbar-widget".into())
        .spawn(widget_thread)
    {
        eprintln!("Failed to spawn taskbar widget thread: {error}");
    }
}

pub fn apply_settings<R: Runtime>(_app: &AppHandle<R>, _settings: &AppSettings) {
    refresh_model(None);
    let hwnd = hwnd_widget();
    if !hwnd.0.is_null() {
        unsafe {
            position_widget(hwnd);
        }
    }
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
    let account = active_id.and_then(|id| store.accounts.iter().find(|account| account.id == id));
    if let Ok(mut model) = MODEL.lock() {
        if model.account_id.as_deref() != active_id {
            clear_usage(&mut model);
        }
        model.account_id = active_id.map(str::to_owned);
        model.enabled = settings.taskbar.enabled;
        model.layout = settings.taskbar.layout;
        model.width = settings
            .taskbar
            .width
            .clamp(TASKBAR_MIN_WIDTH, TASKBAR_MAX_WIDTH);
        model.offset_x = settings.taskbar.offset_x;
        model.offset_y = settings.taskbar.offset_y;
        let language = settings.language.as_str();
        model.chinese = language.starts_with("zh")
            || (language == crate::types::AppLanguage::SYSTEM_CODE
                && sys_locale::get_locale().is_some_and(|locale| locale.starts_with("zh")));
        model.account = account
            .map(|item| item.name.clone())
            .unwrap_or_else(|| "--".into());
        if let Some(usage) = usage {
            apply_active_usage(&mut model, usage, active_id);
        }
    }
}

fn clear_usage(model: &mut WidgetModel) {
    model.primary = None;
    model.secondary = None;
    model.has_primary_window = false;
    model.has_secondary_window = false;
    model.primary_window_minutes = None;
    model.primary_resets_at = None;
    model.secondary_resets_at = None;
}

fn apply_active_usage(model: &mut WidgetModel, usage: &UsageInfo, active_id: Option<&str>) {
    if Some(usage.account_id.as_str()) != active_id {
        return;
    }

    clear_usage(model);
    if usage.error.is_some() {
        return;
    }

    model.primary = remaining(usage.primary_used_percent);
    model.secondary = remaining(usage.secondary_used_percent);
    model.has_primary_window = usage.primary_used_percent.is_some()
        || usage.primary_window_minutes.is_some()
        || usage.primary_resets_at.is_some();
    model.has_secondary_window = usage.secondary_used_percent.is_some()
        || usage.secondary_window_minutes.is_some()
        || usage.secondary_resets_at.is_some();
    model.primary_window_minutes = usage.primary_window_minutes;
    model.primary_resets_at = usage.primary_resets_at;
    model.secondary_resets_at = usage.secondary_resets_at;
}

fn remaining(used: Option<f64>) -> Option<f64> {
    used.filter(|value| value.is_finite())
        .map(|value| (100.0 - value).clamp(0.0, 100.0))
}

fn invalidate() {
    let hwnd = hwnd_widget();
    if !hwnd.0.is_null() {
        unsafe {
            let _ = InvalidateRect(hwnd, None, false);
        }
    }
}

fn hwnd_widget() -> HWND {
    HWND(HWND_WIDGET.load(Ordering::Relaxed) as *mut core::ffi::c_void)
}

fn hwnd_tooltip() -> HWND {
    HWND(HWND_TOOLTIP.load(Ordering::Relaxed) as *mut core::ffi::c_void)
}

fn current_account_name() -> String {
    MODEL
        .lock()
        .map(|model| model.account.clone())
        .unwrap_or_else(|error| error.into_inner().account.clone())
}

fn widget_thread() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let module = match GetModuleHandleW(None) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("Taskbar widget: GetModuleHandleW failed: {error}");
                return;
            }
        };
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
            eprintln!(
                "Taskbar widget: RegisterClassW failed: {}",
                GetLastError().0
            );
            update_last_error(Some("Could not register the native taskbar widget window."));
            return;
        }
        TASKBAR_CREATED_MESSAGE.store(
            RegisterWindowMessageW(w!("TaskbarCreated")),
            Ordering::Relaxed,
        );

        let mut last_attach = Instant::now() - Duration::from_secs(5);
        let mut last_countdown_refresh = Instant::now();
        let mut attach_failures = 0u8;
        let mut last_tooltip_text = String::new();
        loop {
            let current = hwnd_widget();
            let enabled = MODEL.lock().map(|model| model.enabled).unwrap_or(false);
            if enabled
                && (!IsWindow(current).as_bool() || current.0.is_null())
                && last_attach.elapsed() >= POSITION_REFRESH_INTERVAL
            {
                last_attach = Instant::now();
                if attach(instance) {
                    attach_failures = 0;
                    last_tooltip_text = current_account_name();
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
            if !current.0.is_null() {
                let tooltip_text = current_account_name();
                if tooltip_text != last_tooltip_text {
                    update_tooltip(hwnd_tooltip(), current, &tooltip_text);
                    last_tooltip_text = tooltip_text;
                }
            } else {
                last_tooltip_text.clear();
            }

            let mut msg = MSG::default();
            while windows::Win32::UI::WindowsAndMessaging::PeekMessageW(
                &mut msg,
                None,
                0,
                0,
                windows::Win32::UI::WindowsAndMessaging::PM_REMOVE,
            )
            .as_bool()
            {
                if msg.message == windows::Win32::UI::WindowsAndMessaging::WM_QUIT {
                    return;
                }
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

fn taskbar_width(dpi: i32) -> i32 {
    let logical_width = MODEL
        .lock()
        .map(|model| model.width)
        .unwrap_or(TASKBAR_MIN_WIDTH)
        .clamp(TASKBAR_MIN_WIDTH, TASKBAR_MAX_WIDTH);
    logical_width * dpi / 96
}

unsafe fn attach(instance: windows::Win32::Foundation::HINSTANCE) -> bool {
    let taskbar = FindWindowW(w!("Shell_TrayWnd"), None).unwrap_or_default();
    if taskbar.0.is_null() {
        eprintln!("Taskbar widget: Shell_TrayWnd not found");
        return false;
    }
    let notify = FindWindowExW(taskbar, None, w!("TrayNotifyWnd"), None).unwrap_or_default();
    if notify.0.is_null() {
        eprintln!("Taskbar widget: TrayNotifyWnd not found");
        return false;
    }

    let mut taskbar_rect = RECT::default();
    let mut notify_rect = RECT::default();
    if GetWindowRect(taskbar, &mut taskbar_rect).is_err()
        || GetWindowRect(notify, &mut notify_rect).is_err()
    {
        return false;
    }
    if taskbar_rect.bottom < notify_rect.bottom || taskbar_rect.bottom - taskbar_rect.top > 100 {
        return false;
    }

    let dpi = GetDpiForWindow(taskbar).max(96) as i32;
    let width = taskbar_width(dpi);
    let height = taskbar_rect.bottom - taskbar_rect.top;
    let anchor_left = left_edge_before_notify(taskbar_rect, notify_rect);
    let (offset_x, offset_y) = MODEL
        .lock()
        .map(|model| (model.offset_x, model.offset_y))
        .unwrap_or_default();
    let x = anchor_left - width - (12 * dpi / 96) + offset_x;
    if x < taskbar_rect.left {
        return false;
    }

    let background = sample_taskbar_background(x - 1, taskbar_rect.top + height / 2);
    BACKGROUND_KEY.store(background.0, Ordering::Relaxed);
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
        CLASS_NAME,
        w!("Codex Usage"),
        WS_POPUP | WS_VISIBLE,
        x,
        taskbar_rect.top + offset_y,
        width,
        height,
        None,
        HMENU(core::ptr::null_mut()),
        instance,
        None,
    )
    .unwrap_or_default();
    if hwnd.0.is_null() {
        eprintln!(
            "Taskbar widget: CreateWindowExW failed: {}",
            GetLastError().0
        );
        return false;
    }
    let set_parent_result = SetParent(hwnd, taskbar);
    let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
    let _ = SetWindowLongW(hwnd, GWL_STYLE, ((style & !WS_POPUP.0) | WS_CHILD.0) as i32);
    let actual_parent = GetParent(hwnd).unwrap_or_default();
    if actual_parent != taskbar {
        eprintln!(
            "Taskbar widget: SetParent failed: result={set_parent_result:?}, error={}",
            GetLastError().0
        );
        let _ = DestroyWindow(hwnd);
        return false;
    }
    if SetLayeredWindowAttributes(hwnd, background, 255, LWA_COLORKEY).is_err() {
        eprintln!(
            "Taskbar widget: failed to set color key: {}",
            GetLastError().0
        );
        let _ = DestroyWindow(hwnd);
        return false;
    }
    let dark = is_dark_mode();
    LAST_DARK_MODE.store(if dark { 1 } else { 2 }, Ordering::Relaxed);
    HWND_WIDGET.store(hwnd.0 as isize, Ordering::Relaxed);
    create_tooltip(hwnd, instance, &current_account_name());
    position_widget(hwnd);
    let _ = InvalidateRect(hwnd, None, false);
    true
}

unsafe fn create_tooltip(widget: HWND, instance: HINSTANCE, text: &str) {
    let tooltip = CreateWindowExW(
        WS_EX_TOPMOST,
        TOOLTIPS_CLASS,
        PCWSTR::null(),
        WS_POPUP
            | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(TTS_ALWAYSTIP | TTS_NOPREFIX),
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        widget,
        HMENU(core::ptr::null_mut()),
        instance,
        None,
    )
    .unwrap_or_default();
    if tooltip.0.is_null() {
        eprintln!("Taskbar widget: could not create account tooltip");
        return;
    }

    let text = encode_tooltip_text(text);
    if let Ok(mut current_text) = TOOLTIP_TEXT.lock() {
        *current_text = text;
        let mut tool = tooltip_info(widget, current_text.as_mut_slice());
        let _ = SendMessageW(
            tooltip,
            TTM_ADDTOOLW,
            WPARAM(0),
            LPARAM((&mut tool as *mut TTTOOLINFOW).cast::<core::ffi::c_void>() as isize),
        );
        let _ = SendMessageW(tooltip, TTM_SETMAXTIPWIDTH, WPARAM(0), LPARAM(480));
    }
    HWND_TOOLTIP.store(tooltip.0 as isize, Ordering::Relaxed);
}

fn encode_tooltip_text(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn tooltip_info(widget: HWND, text: &mut [u16]) -> TTTOOLINFOW {
    TTTOOLINFOW {
        cbSize: mem::size_of::<TTTOOLINFOW>() as u32,
        uFlags: TTF_IDISHWND | TTF_SUBCLASS,
        hwnd: widget,
        uId: widget.0 as usize,
        lpszText: PWSTR(text.as_mut_ptr()),
        ..Default::default()
    }
}

unsafe fn update_tooltip(tooltip: HWND, widget: HWND, text: &str) {
    if tooltip.0.is_null() || widget.0.is_null() {
        return;
    }

    let next_text = encode_tooltip_text(text);
    if let Ok(mut current_text) = TOOLTIP_TEXT.lock() {
        let previous_text = mem::replace(&mut *current_text, next_text);
        let mut tool = tooltip_info(widget, current_text.as_mut_slice());
        let _ = SendMessageW(
            tooltip,
            TTM_UPDATETIPTEXTW,
            WPARAM(0),
            LPARAM((&mut tool as *mut TTTOOLINFOW).cast::<core::ffi::c_void>() as isize),
        );
        drop(previous_text);
    }
}

fn update_last_error(error: Option<&str>) {
    let mut settings = load_app_settings().unwrap_or_default();
    let next = error.map(str::to_string);
    if settings.taskbar.last_error == next {
        return;
    }
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
    if taskbar.0.is_null() || notify.0.is_null() {
        let _ = DestroyWindow(hwnd);
        return;
    }
    let mut taskbar_rect = RECT::default();
    let mut notify_rect = RECT::default();
    let _ = GetWindowRect(taskbar, &mut taskbar_rect);
    let _ = GetWindowRect(notify, &mut notify_rect);
    let dpi = GetDpiForWindow(taskbar).max(96) as i32;
    let width = taskbar_width(dpi);
    let anchor_left = left_edge_before_notify(taskbar_rect, notify_rect);
    let (enabled, offset_x, offset_y) = MODEL
        .lock()
        .map(|model| (model.enabled, model.offset_x, model.offset_y))
        .unwrap_or((false, 0, 0));
    let x = anchor_left - width - (12 * dpi / 96) + offset_x;
    let y = taskbar_rect.top + offset_y;
    let height = taskbar_rect.bottom - taskbar_rect.top;
    let mut current_rect = RECT::default();
    let _ = GetWindowRect(hwnd, &mut current_rect);
    if current_rect.left != x
        || current_rect.top != y
        || current_rect.right - current_rect.left != width
        || current_rect.bottom - current_rect.top != height
    {
        let _ = SetWindowPos(
            hwnd,
            None,
            x - taskbar_rect.left,
            offset_y,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
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
    if color.0 == u32::MAX {
        fallback_background()
    } else {
        color
    }
}

fn fallback_background() -> COLORREF {
    if is_dark_mode() {
        COLORREF(0x00202020)
    } else {
        COLORREF(0x00F3F3F3)
    }
}

extern "system" fn wnd_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if message == TASKBAR_CREATED_MESSAGE.load(Ordering::Relaxed) {
            position_widget(hwnd);
            return LRESULT(0);
        }
        match message {
            WM_PAINT => {
                paint(hwnd);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_LBUTTONDBLCLK => {
                handle_double_click();
                position_widget(hwnd);
                LRESULT(0)
            }
            WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MOUSEWHEEL => {
                LRESULT(0)
            }
            WM_DESTROY => {
                let tooltip = hwnd_tooltip();
                if !tooltip.0.is_null() {
                    let _ = DestroyWindow(tooltip);
                    HWND_TOOLTIP.store(0, Ordering::Relaxed);
                }
                HWND_WIDGET.store(0, Ordering::Relaxed);
                LRESULT(0)
            }
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
    if width <= 0 || height <= 0 {
        return;
    }

    let memory_dc = CreateCompatibleDC(hdc);
    if memory_dc.0.is_null() {
        return;
    }
    let bitmap = CreateCompatibleBitmap(hdc, width, height);
    if bitmap.0.is_null() {
        let _ = DeleteDC(memory_dc);
        return;
    }
    let old_bitmap = SelectObject(memory_dc, bitmap);
    let background = COLORREF(BACKGROUND_KEY.load(Ordering::Relaxed));
    let brush = CreateSolidBrush(background);
    let _ = FillRect(memory_dc, &rect, brush);
    let _ = DeleteObject(brush);

    let dark = is_dark_mode();
    let foreground = if dark { 0x00F2F2F2 } else { 0x001A1A1A };
    let dpi = GetDpiForWindow(hwnd).max(96) as i32;
    let font = CreateFontW(
        -(9 * dpi / 72),
        0,
        0,
        0,
        FW_NORMAL.0 as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        0,
        0,
        DEFAULT_QUALITY.0 as u32,
        0,
        w!("Microsoft YaHei"),
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
        let mut weekly_rect = RECT {
            left: first_left,
            top: content_top,
            right: second_left - column_gap,
            bottom: content_top + content_height,
        };
        let mut reset_rect = RECT {
            left: second_left,
            top: content_top,
            right,
            bottom: content_top + row_height,
        };
        let mut account_rect = RECT {
            left: second_left,
            top: content_top + row_height,
            right,
            bottom: content_top + content_height,
        };
        draw_left(&mut weekly_rect, &weekly, memory_dc);
        draw_left(&mut reset_rect, &reset, memory_dc);
        let account_width = reserve_account_text_width(&mut account_rect, dpi);
        let account = fit_account_name(&account, account_width, memory_dc);
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
        let mut top_first = RECT {
            left: first_left,
            top: content_top,
            right: second_left - column_gap,
            bottom: content_top + row_height,
        };
        let mut top_second = RECT {
            left: second_left,
            top: content_top,
            right,
            bottom: content_top + row_height,
        };
        let mut bottom_first = RECT {
            left: first_left,
            top: content_top + row_height,
            right: second_left - column_gap,
            bottom: content_top + content_height,
        };
        let mut bottom_second = RECT {
            left: second_left,
            top: content_top + row_height,
            right,
            bottom: content_top + content_height,
        };
        draw_left(&mut top_first, &top_left, memory_dc);
        draw_left(&mut top_second, &top_right, memory_dc);
        draw_left(&mut bottom_first, &bottom_left, memory_dc);
        let account_width = reserve_account_text_width(&mut bottom_second, dpi);
        let account = fit_account_name(&bottom_right, account_width, memory_dc);
        draw_left(&mut bottom_second, &account, memory_dc);
    } else if layout == TaskbarLayout::Compact {
        let mut line = rect;
        draw(&mut line, &line1, memory_dc);
    } else {
        // TrafficMonitor renders its two rows as one compact block rather than
        // splitting the full taskbar height into two large halves.
        let row_height = 16 * dpi / 96;
        let content_height = row_height * 2;
        let content_top = ((rect.bottom - rect.top - content_height) / 2).max(0);
        let mut top = RECT {
            top: content_top,
            bottom: content_top + row_height,
            ..rect
        };
        let mut bottom = RECT {
            top: content_top + row_height,
            bottom: content_top + content_height,
            ..rect
        };
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

fn reserve_account_text_width(rect: &mut RECT, dpi: i32) -> i32 {
    let margin = (ACCOUNT_SAFE_MARGIN_PX * dpi / 96).max(4);
    rect.right = rect.right.saturating_sub(margin).max(rect.left);
    rect.right - rect.left
}

unsafe fn draw(rect: &mut RECT, value: &str, hdc: windows::Win32::Graphics::Gdi::HDC) {
    if value.is_empty() {
        return;
    }
    let mut wide: Vec<u16> = value.encode_utf16().collect();
    let _ = DrawTextW(
        hdc,
        &mut wide,
        rect,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
    );
}

unsafe fn draw_left(rect: &mut RECT, value: &str, hdc: windows::Win32::Graphics::Gdi::HDC) {
    if value.is_empty() {
        return;
    }
    let mut wide: Vec<u16> = value.encode_utf16().collect();
    let _ = DrawTextW(
        hdc,
        &mut wide,
        rect,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
    );
}

fn fit_account_name(
    value: &str,
    max_width: i32,
    hdc: windows::Win32::Graphics::Gdi::HDC,
) -> String {
    let prefix = if value.starts_with("账号: ") {
        "账号: "
    } else if value.starts_with("账号：") {
        "账号："
    } else {
        ""
    };
    let name = value.strip_prefix(prefix).unwrap_or(value);
    let measure_text = |candidate: &str| {
        let wide = candidate.encode_utf16().collect::<Vec<_>>();
        let mut size = SIZE::default();
        unsafe {
            let _ = GetTextExtentPoint32W(hdc, &wide, &mut size);
        }
        size.cx
    };
    let prefix_width = measure_text(prefix);
    if prefix_width >= max_width {
        return middle_ellipsis(value, max_width, measure_text);
    }
    let fitted_name = middle_ellipsis(name, max_width - prefix_width, measure_text);
    format!("{prefix}{fitted_name}")
}

fn middle_ellipsis<F>(value: &str, max_width: i32, measure: F) -> String
where
    F: Fn(&str) -> i32,
{
    if value.is_empty() {
        return value.to_string();
    }
    if max_width <= 0 {
        return String::new();
    }
    if measure(value) <= max_width {
        return value.to_string();
    }

    let chars = value.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return String::new();
    }

    if let Some(at) = chars.iter().position(|character| *character == '@') {
        if at > 0 && at + 1 < chars.len() {
            let suffix = chars[at..].iter().collect::<String>();
            let suffix_with_ellipsis = format!("…{suffix}");
            if measure(&suffix_with_ellipsis) <= max_width {
                let mut best = suffix_with_ellipsis.clone();
                for count in 1..=at {
                    let prefix = chars[..count].iter().collect::<String>();
                    let candidate = format!("{prefix}{suffix_with_ellipsis}");
                    if measure(&candidate) <= max_width {
                        best = candidate;
                    } else {
                        break;
                    }
                }
                return best;
            }
        }
    }

    if measure("…") > max_width {
        return String::new();
    }
    for kept in (1..chars.len()).rev() {
        let head_count = kept.div_ceil(2);
        let tail_count = kept / 2;
        let mut candidate = chars[..head_count].iter().collect::<String>();
        candidate.push('…');
        candidate.extend(chars[chars.len() - tail_count..].iter().copied());
        if measure(&candidate) <= max_width {
            return candidate;
        }
    }
    "…".to_string()
}

fn formatted_detailed_cells() -> [String; 4] {
    let model = MODEL.lock().unwrap_or_else(|error| error.into_inner());
    let primary = model
        .primary
        .map(|value| format!("{value:.0}%"))
        .unwrap_or_else(|| "--".into());
    let secondary = model
        .secondary
        .map(|value| format!("{value:.0}%"))
        .unwrap_or_else(|| "--".into());
    let weekly_only = !model.has_primary_window && model.has_secondary_window;
    let reset = reset_label(
        if weekly_only {
            model.secondary_resets_at
        } else {
            model.primary_resets_at
        },
        model.chinese,
    );
    let primary_label = primary_window_label(model.primary_window_minutes, model.chinese);
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
            format!("{primary_label}: {primary}"),
            format!("重置: {reset}"),
            if model.has_secondary_window {
                format!("周: {secondary}")
            } else {
                String::new()
            },
            format!("账号: {}", model.account),
        ]
    } else {
        [
            format!("{primary_label}: {primary}"),
            format!("Reset: {reset}"),
            if model.has_secondary_window {
                format!("Week: {secondary}")
            } else {
                String::new()
            },
            model.account.clone(),
        ]
    }
}

fn formatted_lines() -> (TaskbarLayout, String, String, bool) {
    let model = MODEL.lock().unwrap_or_else(|error| error.into_inner());
    let p = model
        .primary
        .map(|v| format!("{v:.0}%"))
        .unwrap_or_else(|| "--".into());
    let s = model
        .secondary
        .map(|v| format!("{v:.0}%"))
        .unwrap_or_else(|| "--".into());
    let weekly_only = !model.has_primary_window && model.has_secondary_window;
    let reset = reset_label(
        if weekly_only {
            model.secondary_resets_at
        } else {
            model.primary_resets_at
        },
        model.chinese,
    );
    let primary_label = primary_window_label(model.primary_window_minutes, model.chinese);

    if weekly_only {
        return match model.layout {
            TaskbarLayout::Detailed if model.chinese => (
                model.layout,
                format!("周 {s}"),
                format!("重置 {reset}"),
                true,
            ),
            TaskbarLayout::Detailed => (
                model.layout,
                format!("Week {s}"),
                format!("Reset {reset}"),
                true,
            ),
            TaskbarLayout::Minimal if model.chinese => (
                model.layout,
                format!("周：{s}"),
                format!("重置：{reset}"),
                false,
            ),
            TaskbarLayout::Minimal => (
                model.layout,
                format!("Week: {s}"),
                format!("Reset: {reset}"),
                false,
            ),
            TaskbarLayout::Compact if model.chinese => (
                model.layout,
                format!("周 {s}  ·  {reset}"),
                String::new(),
                false,
            ),
            TaskbarLayout::Compact => (
                model.layout,
                format!("W {s}  ·  {reset}"),
                String::new(),
                false,
            ),
        };
    }

    match model.layout {
        TaskbarLayout::Detailed if model.chinese => (
            model.layout,
            format!("{primary_label}：{p}  重置：{reset}"),
            if model.has_secondary_window {
                format!("周：{s}  账号：{}", model.account)
            } else {
                format!("账号：{}", model.account)
            },
            false,
        ),
        TaskbarLayout::Detailed => (
            model.layout,
            format!("{primary_label}: {p}  Reset: {reset}"),
            if model.has_secondary_window {
                format!("Week: {s}  {}", model.account)
            } else {
                model.account.clone()
            },
            false,
        ),
        TaskbarLayout::Minimal if model.chinese => (
            model.layout,
            format!("{primary_label}：{p} · {reset}"),
            if model.has_secondary_window {
                format!("周：{s}")
            } else {
                String::new()
            },
            false,
        ),
        TaskbarLayout::Minimal => (
            model.layout,
            format!("{primary_label}: {p} · {reset}"),
            if model.has_secondary_window {
                format!("Week: {s}")
            } else {
                String::new()
            },
            false,
        ),
        TaskbarLayout::Compact if model.chinese => (
            model.layout,
            if model.has_secondary_window {
                format!("{primary_label} {p} · 周 {s} · {reset}")
            } else {
                format!("{primary_label} {p} · {reset}")
            },
            String::new(),
            false,
        ),
        TaskbarLayout::Compact => (
            model.layout,
            if model.has_secondary_window {
                format!("{primary_label} {p} · W {s} · {reset}")
            } else {
                format!("{primary_label} {p} · {reset}")
            },
            String::new(),
            false,
        ),
    }
}

fn primary_window_label(window_minutes: Option<i64>, chinese: bool) -> &'static str {
    if window_minutes.is_some_and(|minutes| minutes >= MONTHLY_WINDOW_MINUTES_THRESHOLD) {
        if chinese {
            "每月"
        } else {
            "Month"
        }
    } else {
        "5H"
    }
}

fn reset_label(timestamp: Option<i64>, chinese: bool) -> String {
    reset_label_at(timestamp, chrono::Utc::now().timestamp(), chinese)
}

fn reset_label_at(timestamp: Option<i64>, now: i64, chinese: bool) -> String {
    let Some(timestamp) = timestamp else {
        return "--".into();
    };
    let remaining_seconds = timestamp - now;
    if remaining_seconds < 60 {
        return if chinese {
            "现在".into()
        } else {
            "Now".into()
        };
    }

    if remaining_seconds >= 24 * 60 * 60 {
        let days = remaining_seconds / (24 * 60 * 60);
        if chinese {
            format!("{days}天")
        } else {
            format!("{days}d")
        }
    } else if remaining_seconds >= 60 * 60 {
        let hours = remaining_seconds / (60 * 60);
        if chinese {
            format!("{hours}小时")
        } else {
            format!("{hours}h")
        }
    } else {
        let minutes = remaining_seconds / 60;
        if chinese {
            format!("{minutes}分钟")
        } else {
            format!("{minutes}m")
        }
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
    let Some(app) = APP.get() else {
        return;
    };
    let action = load_app_settings()
        .unwrap_or_default()
        .taskbar
        .double_click_action;
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
    fn middle_ellipsis_preserves_email_domain() {
        let result = middle_ellipsis("very-long-account@example.com", 17, |value| {
            value.chars().count() as i32
        });
        assert_eq!(result, "very…@example.com");
    }

    #[test]
    fn middle_ellipsis_preserves_both_ends_for_regular_names() {
        let result = middle_ellipsis("海外开发测试账号", 6, |value| {
            value.chars().count() as i32
        });
        assert_eq!(result, "海外开…账号");
    }

    #[test]
    fn middle_ellipsis_returns_full_name_when_it_fits() {
        assert_eq!(
            middle_ellipsis("work", 10, |value| value.chars().count() as i32),
            "work"
        );
    }

    #[test]
    fn active_usage_error_clears_stale_quota() {
        let mut model = WidgetModel {
            account_id: Some("account-1".into()),
            primary: Some(64.0),
            secondary: Some(38.0),
            has_primary_window: true,
            has_secondary_window: true,
            primary_resets_at: Some(1_800_000_000),
            secondary_resets_at: Some(1_800_100_000),
            ..WidgetModel::default()
        };
        let usage = UsageInfo::error("account-1".into(), "subscription expired".into());

        apply_active_usage(&mut model, &usage, Some("account-1"));

        assert_eq!(model.primary, None);
        assert_eq!(model.secondary, None);
        assert!(!model.has_primary_window);
        assert!(!model.has_secondary_window);
        assert_eq!(model.primary_resets_at, None);
        assert_eq!(model.secondary_resets_at, None);
        assert_eq!(model.primary_window_minutes, None);
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
            model.secondary_resets_at =
                Some(chrono::Utc::now().timestamp() + 3 * 24 * 60 * 60 + 60 * 60);
            model.account = "work".into();
        }

        let (_, _, _, weekly_only) = formatted_lines();
        assert!(weekly_only);

        let cells = formatted_detailed_cells();
        assert_eq!(cells, ["Week: 65%", "Reset: 3d", "", "work"]);
        assert!(cells.iter().all(|cell| !cell.contains("5H")));
    }

    #[test]
    fn monthly_primary_window_is_labeled_as_monthly() {
        {
            let mut model = MODEL.lock().unwrap_or_else(|error| error.into_inner());
            model.layout = TaskbarLayout::Detailed;
            model.chinese = true;
            model.primary = Some(92.0);
            model.secondary = None;
            model.has_primary_window = true;
            model.has_secondary_window = false;
            model.primary_window_minutes = Some(30 * 24 * 60);
            model.primary_resets_at = Some(chrono::Utc::now().timestamp() + 2 * 24 * 60 * 60);
            model.secondary_resets_at = None;
            model.account = "free".into();
        }

        let (_, first, _, weekly_only) = formatted_lines();
        assert!(!weekly_only);
        assert!(first.starts_with("每月："));
        assert!(!first.contains("5H"));
    }

    #[test]
    fn reset_labels_use_adaptive_localized_units() {
        let now = 1_800_000_000;
        assert_eq!(
            reset_label_at(Some(now + 3 * 24 * 60 * 60 + 4 * 60 * 60), now, true),
            "3天"
        );
        assert_eq!(
            reset_label_at(Some(now + 4 * 60 * 60 + 27 * 60), now, true),
            "4小时"
        );
        assert_eq!(reset_label_at(Some(now + 27 * 60), now, true), "27分钟");
        assert_eq!(reset_label_at(Some(now), now, true), "现在");
        assert_eq!(
            reset_label_at(Some(now + 3 * 24 * 60 * 60), now, false),
            "3d"
        );
        assert_eq!(reset_label_at(Some(now + 4 * 60 * 60), now, false), "4h");
        assert_eq!(reset_label_at(Some(now + 27 * 60), now, false), "27m");
        assert_eq!(reset_label_at(Some(now), now, false), "Now");
    }

    #[test]
    fn reset_labels_do_not_roll_up_before_unit_boundaries() {
        let now = 1_800_000_000;
        assert_eq!(
            reset_label_at(Some(now + 24 * 60 * 60 - 1), now, true),
            "23小时"
        );
        assert_eq!(reset_label_at(Some(now + 60 * 60 - 1), now, true), "59分钟");
        assert_eq!(reset_label_at(Some(now + 59), now, true), "现在");
    }
}
