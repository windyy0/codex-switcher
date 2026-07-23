use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use tauri::{
    menu::{CheckMenuItemBuilder, Menu, MenuItemBuilder, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Runtime,
};

use crate::{
    auth::{get_account, get_accounts_file, load_accounts, load_app_settings},
    commands::{fetch_usage_cached, restore_main_window, warmup_account},
    types::{AccountsStore, AppSettings, AuthMode, StoredAccount, TrayDisplayMode, UsageInfo},
};

static TRAY_USAGE: LazyLock<Mutex<HashMap<String, UsageInfo>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static PENDING_SWITCH_REQUEST: LazyLock<Mutex<Option<SwitchAccountRequestedPayload>>> =
    LazyLock::new(|| Mutex::new(None));

const TRAY_ID: &str = "codex-switcher-tray";
#[cfg(not(target_os = "macos"))]
const TRAY_ICON: tauri::image::Image<'static> = tauri::include_image!("./icons/tray.png");
#[cfg(target_os = "macos")]
const TRAY_ICON: tauri::image::Image<'static> = tauri::include_image!("./icons/tray-template.png");
const ACCOUNTS_CHANGED_EVENT: &str = "accounts-changed";
const SWITCH_ACCOUNT_REQUESTED_EVENT: &str = "tray-switch-account-requested";
const ACCOUNT_USAGE_UPDATED_EVENT: &str = "account-usage-updated";
const OPEN_PAGE_EVENT: &str = "tray-open-page";
const ACCOUNT_ITEM_PREFIX: &str = "account:";
const OPEN_ITEM_ID: &str = "open";
const MANAGE_ACCOUNTS_ITEM_ID: &str = "manage-accounts";
const SETTINGS_ITEM_ID: &str = "settings";
const FLOATING_SETTINGS_ITEM_ID: &str = "floating-settings";
const REFRESH_ACTIVE_ITEM_ID: &str = "refresh-active";
const WARMUP_ACTIVE_ITEM_ID: &str = "warmup-active";
const QUIT_ITEM_ID: &str = "quit";
const FLOATING_VISIBLE_ID: &str = "floating-visible";
const TASKBAR_VISIBLE_ID: &str = "taskbar-visible";
const MAX_RECENT_ACCOUNTS: usize = 8;
const MAX_MENU_ACCOUNT_NAME_CHARS: usize = 28;

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchAccountRequestedPayload {
    account_id: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum TrayAccountSwitchStatus {
    Switched,
    Blocked,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrayAccountSwitchOutcome {
    status: TrayAccountSwitchStatus,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountUsageUpdatedPayload {
    usage: UsageInfo,
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let store = load_accounts().unwrap_or_default();
    let settings = load_app_settings().unwrap_or_default();
    let menu = build_menu(app, &store, &settings)?;

    #[cfg(target_os = "linux")]
    let icon = app
        .default_window_icon()
        .cloned()
        .expect("application icon should be configured");

    #[cfg(not(target_os = "linux"))]
    let icon = TRAY_ICON;

    let builder = TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("Codex Switcher")
        .menu(&menu)
        .on_menu_event(handle_menu_event);

    #[cfg(target_os = "macos")]
    let builder = builder.icon_as_template(true);

    #[cfg(not(target_os = "linux"))]
    let builder = builder
        .on_tray_icon_event(handle_tray_icon_event)
        .show_menu_on_left_click(false);

    builder.build(app)?;
    refresh_menu(app);

    watch_accounts_file(app.clone());
    poll_active_account_usage(app.clone());
    Ok(())
}

pub fn refresh<R: Runtime>(app: &AppHandle<R>) {
    refresh_menu(app);
}

/// Store usage reported by the main app and refresh the native menu labels.
pub fn ingest_usage<R: Runtime>(app: &AppHandle<R>, usages: Vec<UsageInfo>) {
    #[cfg(target_os = "windows")]
    for usage in &usages {
        crate::taskbar_widget::ingest_usage(usage);
    }
    if let Ok(mut cache) = TRAY_USAGE.lock() {
        for usage in usages {
            cache.insert(usage.account_id.clone(), usage);
        }
    }
    refresh_menu(app);
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
fn handle_tray_icon_event<R: Runtime>(tray: &tauri::tray::TrayIcon<R>, event: TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        show_main_window(tray.app_handle());
    }
}

// ============================================================================
// Native menu (the only tray interaction on Linux; right-click on macOS/Windows)
// ============================================================================

fn build_menu<R: Runtime>(
    app: &AppHandle<R>,
    store: &AccountsStore,
    settings: &AppSettings,
) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;
    let resolved_code = crate::i18n::resolved_code(&settings.language);
    let t = |key| crate::i18n::text_for_code(resolved_code, key);
    let active_account = store
        .active_account_id
        .as_deref()
        .and_then(|id| store.accounts.iter().find(|account| account.id == id));

    if let Some(account) = active_account {
        menu.append(
            &MenuItemBuilder::with_id(
                "current-account-summary",
                format!(
                    "{}: {}",
                    t("currentAccount"),
                    menu_label(&truncate_account_name(&account.name))
                ),
            )
            .enabled(false)
            .build(app)?,
        )?;
        menu.append(
            &MenuItemBuilder::with_id(
                "current-usage-summary",
                active_account_usage_summary(account, resolved_code),
            )
            .enabled(false)
            .build(app)?,
        )?;
    } else {
        menu.append(
            &MenuItemBuilder::with_id("empty", t("noAccounts"))
                .enabled(false)
                .build(app)?,
        )?;
    }

    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItemBuilder::with_id(OPEN_ITEM_ID, t("openApp")).build(app)?)?;

    if !store.accounts.is_empty() {
        let switch_menu = Submenu::new(app, t("switchAccount"), true)?;
        let switch_accounts = recent_switch_accounts(store);
        for account in &switch_accounts {
            let is_active = store.active_account_id.as_deref() == Some(&account.id);
            let account_name = menu_label(&truncate_account_name(&account.name));
            let label = if is_active {
                format!("✓ {account_name}")
            } else {
                account_name
            };
            // Account choices are regular menu items, not native checkboxes. Windows toggles a
            // checkbox before the switch confirmation resolves, which leaves a false second
            // checkmark behind when the user cancels.
            let item = MenuItemBuilder::with_id(account_menu_id(&account.id), label)
                .enabled(!is_active)
                .build(app)?;
            switch_menu.append(&item)?;
        }
        switch_menu.append(&PredefinedMenuItem::separator(app)?)?;
        switch_menu.append(
            &MenuItemBuilder::with_id(MANAGE_ACCOUNTS_ITEM_ID, t("manageAllAccounts"))
                .build(app)?,
        )?;
        menu.append(&switch_menu)?;
    }

    if let Some(account) = active_account {
        let supports_usage_actions = !account.disabled && account.auth_mode == AuthMode::ChatGPT;
        let active_actions = Submenu::new(app, t("currentAccountActions"), true)?;
        active_actions.append(
            &MenuItemBuilder::with_id(REFRESH_ACTIVE_ITEM_ID, t("refreshCurrentAccount"))
                .enabled(supports_usage_actions)
                .build(app)?,
        )?;
        active_actions.append(
            &MenuItemBuilder::with_id(WARMUP_ACTIVE_ITEM_ID, t("warmupCurrentAccount"))
                .enabled(supports_usage_actions)
                .build(app)?,
        )?;
        menu.append(&active_actions)?;
    }

    menu.append(&PredefinedMenuItem::separator(app)?)?;
    #[cfg(target_os = "windows")]
    {
        let display_menu = Submenu::new(app, t("displayComponents"), true)?;
        display_menu.append(
            &CheckMenuItemBuilder::with_id(FLOATING_VISIBLE_ID, t("floatingWindow"))
                .checked(settings.floating.visible)
                .build(app)?,
        )?;
        display_menu.append(
            &CheckMenuItemBuilder::with_id(TASKBAR_VISIBLE_ID, t("taskbarWidget"))
                .checked(settings.taskbar.enabled)
                .build(app)?,
        )?;
        display_menu.append(&PredefinedMenuItem::separator(app)?)?;
        display_menu.append(
            &MenuItemBuilder::with_id(FLOATING_SETTINGS_ITEM_ID, t("floatingSettings"))
                .build(app)?,
        )?;
        menu.append(&display_menu)?;
    }
    #[cfg(target_os = "macos")]
    append_dock_settings_menu(app, &menu, settings, resolved_code)?;
    menu.append(&MenuItemBuilder::with_id(SETTINGS_ITEM_ID, t("settingsEllipsis")).build(app)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItemBuilder::with_id(QUIT_ITEM_ID, t("quitApp")).build(app)?)?;
    Ok(menu)
}

#[cfg(target_os = "macos")]
fn append_dock_settings_menu<R: Runtime>(
    app: &AppHandle<R>,
    menu: &Menu<R>,
    settings: &AppSettings,
    resolved_code: &str,
) -> tauri::Result<()> {
    let t = |key| crate::i18n::text_for_code(resolved_code, key);
    let dock_settings = Submenu::with_items(
        app,
        t("dockIcon"),
        true,
        &[
            &CheckMenuItemBuilder::with_id(crate::app_menu::DOCK_SHOW_IN_DOCK_ID, t("showInDock"))
                .checked(settings.dock_display_mode == crate::app_menu::DockDisplayMode::ShowInDock)
                .build(app)?,
            &CheckMenuItemBuilder::with_id(
                crate::app_menu::DOCK_MENU_BAR_ONLY_ID,
                t("menuBarOnly"),
            )
            .checked(settings.dock_display_mode == crate::app_menu::DockDisplayMode::MenuBarOnly)
            .build(app)?,
        ],
    )?;
    menu.append(&dock_settings)?;
    Ok(())
}

fn handle_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    let item_id = event.id().as_ref();

    #[cfg(target_os = "macos")]
    if let Some(mode) = crate::app_menu::dock_display_mode_for_item(item_id) {
        crate::app_menu::update_dock_display_mode(app, mode);
        return;
    }

    match item_id {
        OPEN_ITEM_ID => show_main_window(app),
        MANAGE_ACCOUNTS_ITEM_ID => show_main_page(app, "accounts"),
        SETTINGS_ITEM_ID | FLOATING_SETTINGS_ITEM_ID => show_main_page(app, "settings"),
        REFRESH_ACTIVE_ITEM_ID => refresh_active_account(app),
        WARMUP_ACTIVE_ITEM_ID => warmup_active_account(),
        QUIT_ITEM_ID => app.exit(0),
        FLOATING_VISIBLE_ID => {
            crate::floating::toggle(app);
            refresh_menu(app);
        }
        TASKBAR_VISIBLE_ID => {
            let mut settings = load_app_settings().unwrap_or_default();
            settings.taskbar.enabled = !settings.taskbar.enabled;
            if crate::auth::save_app_settings(&settings).is_ok() {
                #[cfg(target_os = "windows")]
                crate::taskbar_widget::apply_settings(app, &settings);
                let _ = app.emit(crate::commands::settings::SETTINGS_CHANGED_EVENT, settings);
            }
            refresh_menu(app);
        }
        _ => {
            let Some(account_id) = item_id.strip_prefix(ACCOUNT_ITEM_PREFIX) else {
                return;
            };

            if load_accounts()
                .ok()
                .and_then(|store| store.active_account_id)
                .as_deref()
                == Some(account_id)
            {
                refresh_menu(app);
                return;
            }

            queue_switch_account_request(app, account_id);
        }
    }
}

fn queue_switch_account_request<R: Runtime>(app: &AppHandle<R>, account_id: &str) {
    let payload = SwitchAccountRequestedPayload {
        account_id: account_id.to_string(),
    };

    if let Ok(mut pending) = PENDING_SWITCH_REQUEST.lock() {
        // Only the latest unclaimed choice matters. Once the frontend claims a
        // request it is processed serially, while later clicks remain queued here.
        *pending = Some(payload.clone());
    } else {
        eprintln!("Failed to queue tray account switch request");
        return;
    }

    // This event is only a wake-up signal. The request remains in Rust until the
    // frontend explicitly claims it, so startup-time events cannot be lost.
    let _ = app.emit(SWITCH_ACCOUNT_REQUESTED_EVENT, ());
}

/// Claim the latest account switch requested from the native tray menu.
#[tauri::command]
pub fn take_pending_tray_switch_request() -> Option<SwitchAccountRequestedPayload> {
    PENDING_SWITCH_REQUEST
        .lock()
        .ok()
        .and_then(|mut pending| pending.take())
}

/// Execute a tray switch and return a stable status instead of asking the
/// frontend to parse a localized/backend error message.
#[tauri::command]
pub async fn switch_account_from_tray(
    app: AppHandle,
    account_id: String,
) -> Result<TrayAccountSwitchOutcome, String> {
    let result = tauri::async_runtime::spawn_blocking(move || {
        crate::commands::switch_account_by_id(&account_id)
    })
    .await
    .map_err(|error| format!("Tray account switch task failed: {error}"))?;

    match result {
        Ok(()) => {
            sync_active_account_displays(&app);
            Ok(TrayAccountSwitchOutcome {
                status: TrayAccountSwitchStatus::Switched,
            })
        }
        Err(error) if crate::commands::process::is_codex_running_switch_block(&error) => {
            Ok(TrayAccountSwitchOutcome {
                status: TrayAccountSwitchStatus::Blocked,
            })
        }
        Err(error) => Err(error),
    }
}

fn sync_active_account_displays<R: Runtime>(app: &AppHandle<R>) {
    #[cfg(target_os = "windows")]
    {
        let active_usage = load_accounts()
            .ok()
            .and_then(|store| store.active_account_id)
            .and_then(|account_id| {
                TRAY_USAGE
                    .lock()
                    .ok()
                    .and_then(|cache| cache.get(&account_id).cloned())
            });
        if let Some(usage) = active_usage.as_ref() {
            crate::taskbar_widget::ingest_usage(usage);
        } else {
            crate::taskbar_widget::refresh_active_account();
        }
    }
    refresh_menu(app);
}

fn recent_switch_accounts(store: &AccountsStore) -> Vec<&StoredAccount> {
    let active_id = store.active_account_id.as_deref();
    let mut accounts = store
        .accounts
        .iter()
        .filter(|account| !account.disabled)
        .collect::<Vec<_>>();
    accounts.sort_by(|left, right| {
        let left_is_active = active_id == Some(left.id.as_str());
        let right_is_active = active_id == Some(right.id.as_str());
        right_is_active
            .cmp(&left_is_active)
            .then_with(|| right.last_used_at.cmp(&left.last_used_at))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    accounts.truncate(MAX_RECENT_ACCOUNTS);
    accounts
}

fn truncate_account_name(name: &str) -> String {
    if name.chars().count() <= MAX_MENU_ACCOUNT_NAME_CHARS {
        return name.to_string();
    }

    let mut truncated = name
        .chars()
        .take(MAX_MENU_ACCOUNT_NAME_CHARS.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

fn active_account_usage_summary(account: &StoredAccount, language_code: &str) -> String {
    let t = |key| crate::i18n::text_for_code(language_code, key);
    if account.auth_mode == AuthMode::ApiKey {
        return t("apiUsageManagedExternally").to_string();
    }

    let usage = TRAY_USAGE
        .lock()
        .ok()
        .and_then(|cache| cache.get(&account.id).cloned());
    let Some(usage) = usage.filter(|usage| usage.error.is_none()) else {
        return t("usageUnavailable").to_string();
    };

    let mut parts = Vec::new();
    if let Some(percent) = remaining_percent_label(usage.primary_used_percent) {
        parts.push(t("fiveHourRemaining").replace("{percent}", &percent));
    }
    if let Some(percent) = remaining_percent_label(usage.secondary_used_percent) {
        parts.push(t("weeklyRemaining").replace("{percent}", &percent));
    }
    if parts.is_empty() {
        t("usageUnavailable").to_string()
    } else {
        parts.join(" · ")
    }
}

fn refresh_active_account(app: &AppHandle) {
    let Some(account_id) = load_accounts()
        .ok()
        .and_then(|store| store.active_account_id)
    else {
        return;
    };
    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        match fetch_usage_cached(&account_id, true).await {
            Ok(usage) => {
                ingest_usage(&app_handle, vec![usage.clone()]);
                let _ = app_handle.emit(
                    ACCOUNT_USAGE_UPDATED_EVENT,
                    AccountUsageUpdatedPayload { usage },
                );
            }
            Err(error) => eprintln!("Failed to refresh active account from tray: {error}"),
        }
    });
}

fn warmup_active_account() {
    let Some(account_id) = load_accounts()
        .ok()
        .and_then(|store| store.active_account_id)
    else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        if let Err(error) = warmup_account(account_id).await {
            eprintln!("Failed to warm up active account from tray: {error}");
        }
    });
}

fn refresh_menu<R: Runtime>(app: &AppHandle<R>) {
    let app_handle = app.clone();
    if let Err(error) = app.run_on_main_thread(move || {
        refresh_menu_on_main_thread(&app_handle);
    }) {
        eprintln!("Failed to schedule tray menu refresh: {error}");
    }
}

fn refresh_menu_on_main_thread<R: Runtime>(app: &AppHandle<R>) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };

    match load_accounts()
        .map_err(|error| error.to_string())
        .and_then(|store| {
            let settings = load_app_settings().unwrap_or_default();
            let resolved_code = crate::i18n::resolved_code(&settings.language);
            let title = active_tray_title(
                store.active_account_id.as_deref(),
                settings.tray_display_mode,
            );
            let tooltip = tray_tooltip(&store, resolved_code);
            let menu = build_menu(app, &store, &settings).map_err(|error| error.to_string())?;
            Ok((menu, title, tooltip, settings.tray_display_mode))
        }) {
        Ok((menu, title, tooltip, mode)) => {
            if let Err(error) = tray.set_menu(Some(menu)) {
                eprintln!("Failed to refresh tray menu: {error}");
            }
            refresh_tray_display(&tray, mode, title.as_deref(), &tooltip);
        }
        Err(error) => eprintln!("Failed to build tray menu: {error}"),
    }
}

fn refresh_tray_display<R: Runtime>(
    tray: &tauri::tray::TrayIcon<R>,
    mode: TrayDisplayMode,
    title: Option<&str>,
    tooltip: &str,
) {
    if let Err(error) = tray.set_tooltip(Some(tooltip)) {
        eprintln!("Failed to refresh tray tooltip: {error}");
    }
    match mode {
        TrayDisplayMode::IconAndSession => {
            if let Err(error) = tray.set_visible(true) {
                eprintln!("Failed to show tray icon: {error}");
            }
            #[cfg(not(target_os = "linux"))]
            {
                if let Err(error) = tray.set_icon(Some(TRAY_ICON)) {
                    eprintln!("Failed to refresh tray icon: {error}");
                }
                #[cfg(target_os = "macos")]
                {
                    if let Err(error) = tray.set_icon_as_template(true) {
                        eprintln!("Failed to refresh tray icon template mode: {error}");
                    }
                }
            }
            if let Err(error) = tray.set_title(title) {
                eprintln!("Failed to refresh tray title: {error}");
            }
        }
        TrayDisplayMode::ActiveUsageText => {
            if let Err(error) = tray.set_visible(true) {
                eprintln!("Failed to show tray icon: {error}");
            }
            #[cfg(target_os = "macos")]
            if let Err(error) = tray.set_icon(None) {
                eprintln!("Failed to hide tray icon: {error}");
            }
            #[cfg(target_os = "windows")]
            if let Err(error) = tray.set_icon(Some(TRAY_ICON)) {
                eprintln!("Failed to refresh tray icon: {error}");
            }
            if let Err(error) = tray.set_title(title) {
                eprintln!("Failed to refresh tray title: {error}");
            }
        }
        TrayDisplayMode::Hidden => {
            if let Err(error) = tray.set_title(None::<&str>) {
                eprintln!("Failed to clear tray title: {error}");
            }
            if let Err(error) = tray.set_visible(false) {
                eprintln!("Failed to hide tray icon: {error}");
            }
        }
    }
}

fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    restore_main_window(app);
}

fn show_main_page<R: Runtime>(app: &AppHandle<R>, page: &str) {
    restore_main_window(app);
    let _ = app.emit_to("main", OPEN_PAGE_EVENT, page);
}

fn tray_tooltip(store: &AccountsStore, language_code: &str) -> String {
    let t = |key| crate::i18n::text_for_code(language_code, key);
    let Some(account) = store
        .active_account_id
        .as_deref()
        .and_then(|id| store.accounts.iter().find(|account| account.id == id))
    else {
        return format!("Codex Switcher\n{}", t("noAccounts"));
    };

    format!(
        "Codex Switcher\n{}: {}\n{}",
        t("currentAccount"),
        truncate_account_name(&account.name),
        active_account_usage_summary(account, language_code)
    )
}

// The tray title sits after the icon, e.g. "[icon] 66%".
fn active_session_title(active_account_id: Option<&str>) -> Option<String> {
    let active_account_id = active_account_id?;
    let cache = TRAY_USAGE.lock().ok()?;
    let usage = cache.get(active_account_id)?;
    session_remaining_title(
        usage.primary_used_percent.or(usage.secondary_used_percent),
        usage.error.is_some(),
    )
}

fn active_tray_title(active_account_id: Option<&str>, mode: TrayDisplayMode) -> Option<String> {
    match mode {
        TrayDisplayMode::IconAndSession => active_session_title(active_account_id),
        TrayDisplayMode::ActiveUsageText => Some(active_usage_title(active_account_id)),
        TrayDisplayMode::Hidden => None,
    }
}

fn active_usage_title(active_account_id: Option<&str>) -> String {
    let Some(active_account_id) = active_account_id else {
        return "Codex".to_string();
    };

    let usage = TRAY_USAGE
        .lock()
        .ok()
        .and_then(|cache| cache.get(active_account_id).cloned());

    match usage {
        Some(usage) if usage.error.is_none() => {
            usage_title(usage.primary_used_percent, usage.secondary_used_percent)
        }
        _ => "H:-- W:--".to_string(),
    }
}

fn usage_title(primary_used_percent: Option<f64>, secondary_used_percent: Option<f64>) -> String {
    let mut parts = Vec::new();
    if let Some(remaining) = remaining_percent_label(primary_used_percent) {
        parts.push(format!("H:{remaining}"));
    }
    if let Some(remaining) = remaining_percent_label(secondary_used_percent) {
        parts.push(format!("W:{remaining}"));
    }

    if parts.is_empty() {
        "H:-- W:--".to_string()
    } else {
        parts.join(" ")
    }
}

fn session_remaining_title(used_percent: Option<f64>, has_error: bool) -> Option<String> {
    if has_error {
        return None;
    }

    remaining_percent_label(used_percent)
}

fn remaining_percent_label(used_percent: Option<f64>) -> Option<String> {
    let used_percent = used_percent?;
    if !used_percent.is_finite() {
        return None;
    }

    Some(format!("{:.0}%", (100.0 - used_percent).clamp(0.0, 100.0)))
}

fn account_menu_id(account_id: &str) -> String {
    format!("{ACCOUNT_ITEM_PREFIX}{account_id}")
}

fn menu_label(label: &str) -> String {
    label.replace('&', "&&")
}

// ============================================================================
// Shared: react to external account changes
// ============================================================================

fn watch_accounts_file<R: Runtime>(app: AppHandle<R>) {
    std::thread::spawn(move || {
        let accounts_path = match get_accounts_file() {
            Ok(path) => path,
            Err(error) => {
                eprintln!("Failed to resolve accounts file for tray: {error}");
                return;
            }
        };
        let mut last_modified = modified_at(&accounts_path);

        loop {
            std::thread::sleep(Duration::from_secs(1));
            let modified = modified_at(&accounts_path);
            if modified != last_modified {
                last_modified = modified;
                refresh_menu(&app); // keep the native menu current
                let _ = app.emit(ACCOUNTS_CHANGED_EVENT, ()); // refresh the React UIs
            }
        }
    });
}

fn modified_at(path: &std::path::Path) -> Option<std::time::SystemTime> {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
}

/// Poll the active account's usage so the tray title stays fresh even when the
/// main window's webview poller is hidden or suspended by the OS.
fn poll_active_account_usage<R: Runtime>(app: AppHandle<R>) {
    std::thread::spawn(move || loop {
        let account = load_accounts()
            .ok()
            .and_then(|store| store.active_account_id)
            .and_then(|id| get_account(&id).ok().flatten());

        if let Some(account) =
            account.filter(|account| account.auth_mode == AuthMode::ChatGPT && !account.disabled)
        {
            match tauri::async_runtime::block_on(fetch_usage_cached(&account.id, false)) {
                // Keep the last known title on transient fetch errors.
                Ok(usage) => ingest_usage(&app, vec![usage]),
                Err(error) => eprintln!("Failed to poll usage for tray title: {error}"),
            }
        }

        std::thread::sleep(Duration::from_secs(60));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_switch_payload_and_outcome_have_stable_wire_formats() {
        let request = SwitchAccountRequestedPayload {
            account_id: "account-1".to_string(),
        };
        let outcome = TrayAccountSwitchOutcome {
            status: TrayAccountSwitchStatus::Blocked,
        };

        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({ "accountId": "account-1" })
        );
        assert_eq!(
            serde_json::to_value(outcome).unwrap(),
            serde_json::json!({ "status": "blocked" })
        );
    }

    #[test]
    fn pending_tray_switch_remains_available_until_claimed() {
        *PENDING_SWITCH_REQUEST.lock().unwrap() = Some(SwitchAccountRequestedPayload {
            account_id: "queued-account".to_string(),
        });

        let request = take_pending_tray_switch_request().expect("request should remain queued");
        assert_eq!(request.account_id, "queued-account");
        assert!(take_pending_tray_switch_request().is_none());
    }

    #[test]
    fn embedded_tray_icon_is_not_an_opaque_block() {
        let alphas: Vec<_> = TRAY_ICON
            .rgba()
            .iter()
            .skip(3)
            .step_by(4)
            .copied()
            .collect();
        let width = TRAY_ICON.width() as usize;

        assert_eq!(
            [
                alphas[0],
                alphas[width - 1],
                alphas[alphas.len() - width],
                alphas[alphas.len() - 1]
            ],
            [0, 0, 0, 0]
        );
        assert!(alphas.contains(&0));
        assert!(alphas.contains(&255));
    }

    #[test]
    fn account_ids_are_namespaced_for_tray_events() {
        assert_eq!(account_menu_id("abc-123"), "account:abc-123");
    }

    #[test]
    fn menu_labels_escape_mnemonic_markers() {
        assert_eq!(
            menu_label("Research & Development"),
            "Research && Development"
        );
    }

    #[test]
    fn long_account_names_are_truncated_on_character_boundaries() {
        let name = "这是一个特别特别长的账户别名用来验证托盘菜单不会被无限撑宽";
        let truncated = truncate_account_name(name);

        assert_eq!(truncated.chars().count(), MAX_MENU_ACCOUNT_NAME_CHARS);
        assert!(truncated.ends_with('…'));
        assert!(name.starts_with(truncated.trim_end_matches('…')));
    }

    #[test]
    fn session_title_shows_remaining_percentage() {
        assert_eq!(
            session_remaining_title(Some(34.0), false),
            Some("66%".to_string())
        );
    }

    #[test]
    fn session_title_hides_unknown_or_invalid_usage() {
        assert_eq!(session_remaining_title(None, false), None);
        assert_eq!(session_remaining_title(Some(f64::NAN), false), None);
        assert_eq!(session_remaining_title(Some(34.0), true), None);
    }

    #[test]
    fn session_title_clamps_remaining_percentage() {
        assert_eq!(
            session_remaining_title(Some(-5.0), false),
            Some("100%".to_string())
        );
        assert_eq!(
            session_remaining_title(Some(105.0), false),
            Some("0%".to_string())
        );
    }

    #[test]
    fn usage_title_omits_missing_windows() {
        assert_eq!(usage_title(Some(27.0), Some(82.0)), "H:73% W:18%");
        assert_eq!(usage_title(None, Some(35.0)), "W:65%");
        assert_eq!(usage_title(Some(27.0), None), "H:73%");
        assert_eq!(usage_title(None, None), "H:-- W:--");
    }

    #[test]
    fn active_usage_title_falls_back_when_usage_is_missing() {
        assert_eq!(active_usage_title(Some("missing")), "H:-- W:--");
        assert_eq!(active_usage_title(None), "Codex");
    }

    #[test]
    fn hidden_tray_mode_has_no_title() {
        assert_eq!(
            active_tray_title(Some("active"), TrayDisplayMode::Hidden),
            None
        );
    }
}
