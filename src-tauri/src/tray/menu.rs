//! Keep native menu objects alive across quota polls. On Windows muda allocates
//! monotonically increasing u32 IDs, but WM_COMMAND carries only a u16 ID.
//! Rebuilding the menu every minute eventually makes every click disappear.

use std::collections::HashMap;

use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, MenuItemKind, PredefinedMenuItem, Submenu},
    AppHandle, Runtime,
};

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Entry {
    Item {
        id: String,
        text: String,
        enabled: bool,
    },
    Check {
        id: String,
        text: String,
        checked: bool,
    },
    Submenu {
        id: String,
        text: String,
        children: Vec<Entry>,
    },
    Separator(String),
}

impl Entry {
    pub(super) fn item(id: impl Into<String>, text: impl Into<String>, enabled: bool) -> Self {
        Self::Item {
            id: id.into(),
            text: text.into(),
            enabled,
        }
    }

    pub(super) fn check(id: impl Into<String>, text: impl Into<String>, checked: bool) -> Self {
        Self::Check {
            id: id.into(),
            text: text.into(),
            checked,
        }
    }

    pub(super) fn submenu(
        id: impl Into<String>,
        text: impl Into<String>,
        children: Vec<Self>,
    ) -> Self {
        Self::Submenu {
            id: id.into(),
            text: text.into(),
            children,
        }
    }

    pub(super) fn separator(id: &str) -> Self {
        Self::Separator(id.into())
    }

    fn id(&self) -> &str {
        match self {
            Self::Item { id, .. }
            | Self::Check { id, .. }
            | Self::Submenu { id, .. }
            | Self::Separator(id) => id,
        }
    }
}

// Accessed only from setup / refresh_menu_on_main_thread. Retain account items
// when they leave the recent list: reordering must never allocate or retarget
// command IDs, including when a click from an open menu is already queued.
pub(super) struct NativeMenu<R: Runtime> {
    pub(super) root: Menu<R>,
    items: HashMap<String, MenuItemKind<R>>,
}

impl<R: Runtime> NativeMenu<R> {
    pub(super) fn new(app: &AppHandle<R>) -> tauri::Result<Self> {
        Ok(Self {
            root: Menu::new(app)?,
            items: HashMap::new(),
        })
    }

    pub(super) fn update(&mut self, app: &AppHandle<R>, entries: &[Entry]) -> tauri::Result<()> {
        let desired = self.resolve(app, entries)?;
        let current = self.root.items()?;
        if !same_items(&current, &desired) {
            for item in &current {
                self.root.remove(item)?;
            }
            for item in &desired {
                self.root.append(item)?;
            }
        }
        Ok(())
    }

    fn resolve(
        &mut self,
        app: &AppHandle<R>,
        entries: &[Entry],
    ) -> tauri::Result<Vec<MenuItemKind<R>>> {
        let mut resolved = Vec::with_capacity(entries.len());
        for entry in entries {
            if !self.items.contains_key(entry.id()) {
                let item =
                    match entry {
                        Entry::Item { id, text, enabled } => MenuItemKind::MenuItem(
                            MenuItem::with_id(app, id, text, *enabled, None::<&str>)?,
                        ),
                        Entry::Check { id, text, checked } => MenuItemKind::Check(
                            CheckMenuItem::with_id(app, id, text, true, *checked, None::<&str>)?,
                        ),
                        Entry::Submenu { id, text, .. } => {
                            MenuItemKind::Submenu(Submenu::with_id(app, id, text, true)?)
                        }
                        Entry::Separator(_) => {
                            MenuItemKind::Predefined(PredefinedMenuItem::separator(app)?)
                        }
                    };
                self.items.insert(entry.id().to_owned(), item);
            }
            let item = self.items[entry.id()].clone();
            match (entry, &item) {
                (Entry::Item { text, enabled, .. }, MenuItemKind::MenuItem(item)) => {
                    if item.text()? != *text {
                        item.set_text(text)?;
                    }
                    if item.is_enabled()? != *enabled {
                        item.set_enabled(*enabled)?;
                    }
                }
                (Entry::Check { text, checked, .. }, MenuItemKind::Check(item)) => {
                    if item.text()? != *text {
                        item.set_text(text)?;
                    }
                    // Read the native state: Windows toggles checks before dispatch.
                    if item.is_checked()? != *checked {
                        item.set_checked(*checked)?;
                    }
                }
                (Entry::Submenu { text, children, .. }, MenuItemKind::Submenu(menu)) => {
                    if menu.text()? != *text {
                        menu.set_text(text)?;
                    }
                    let desired = self.resolve(app, children)?;
                    let current = menu.items()?;
                    if !same_items(&current, &desired) {
                        for item in &current {
                            menu.remove(item)?;
                        }
                        for item in &desired {
                            menu.append(item)?;
                        }
                    }
                }
                (Entry::Separator(_), MenuItemKind::Predefined(_)) => {}
                _ => unreachable!("a tray entry must keep its kind for its entire lifetime"),
            }
            resolved.push(item);
        }
        Ok(resolved)
    }
}

fn same_items<R: Runtime>(left: &[MenuItemKind<R>], right: &[MenuItemKind<R>]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.id() == right.id())
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use tauri::menu::ContextMenu;
    use windows::Win32::{
        Foundation::{BOOL, HWND, LPARAM, WPARAM},
        System::Threading::GetCurrentThreadId,
        UI::WindowsAndMessaging::{
            EnumThreadWindows, GetClassNameW, GetMenuItemID, GetSubMenu, SendMessageW, HMENU,
            WM_COMMAND,
        },
    };

    fn command_ids<R: Runtime>(menu: &Menu<R>) -> HashMap<String, u32> {
        fn collect<R: Runtime>(
            handle: HMENU,
            items: Vec<MenuItemKind<R>>,
            ids: &mut HashMap<String, u32>,
        ) {
            for (index, item) in items.into_iter().enumerate() {
                match item {
                    MenuItemKind::Submenu(submenu) => collect(
                        unsafe { GetSubMenu(handle, index as i32) },
                        submenu.items().unwrap(),
                        ids,
                    ),
                    MenuItemKind::MenuItem(_) | MenuItemKind::Check(_) => {
                        let id = unsafe { GetMenuItemID(handle, index as i32) };
                        ids.insert(item.id().as_ref().to_owned(), id);
                    }
                    _ => {}
                }
            }
        }
        let mut ids = HashMap::new();
        collect(
            HMENU(menu.hpopupmenu().unwrap() as _),
            menu.items().unwrap(),
            &mut ids,
        );
        ids
    }

    // Only inspect this test thread's native tray window, never another app.
    unsafe extern "system" fn find_test_tray(hwnd: HWND, output: LPARAM) -> BOOL {
        let mut class = [0u16; 64];
        let len = GetClassNameW(hwnd, &mut class);
        if String::from_utf16_lossy(&class[..len as usize]) == "tray_icon_app" {
            *(output.0 as *mut HWND) = hwnd;
        }
        true.into()
    }

    #[test]
    fn native_commands_survive_ten_thousand_refreshes_and_reproduce_legacy_overflow() {
        use crate::{
            tray,
            types::{AccountsStore, AppLanguage, AppSettings, StoredAccount, UsageInfo},
        };
        // muda's global handler is a OnceCell. Install our observer before the
        // mock app installs its (intentionally non-delivering) runtime proxy.
        let (tx, rx) = std::sync::mpsc::channel();
        muda::MenuEvent::set_event_handler(Some(move |event: muda::MenuEvent| {
            tx.send(event.id.0).unwrap();
        }));
        let app = tauri::test::mock_app();
        let app = app.handle();
        let mut menu = NativeMenu::new(app).unwrap();
        let mut store = AccountsStore::default();
        for index in 0..10 {
            let mut account =
                StoredAccount::new_api_key(format!("Account {index}"), "test-only".into());
            account.id = format!("native-menu-test-{index}");
            store.accounts.push(account);
        }
        store.active_account_id = Some(store.accounts[0].id.clone());
        let mut settings = AppSettings::default();
        // Warm every account once, including those outside the eight recent entries.
        for index in 0..store.accounts.len() {
            store.active_account_id = Some(store.accounts[index].id.clone());
            menu.update(app, &tray::menu_entries(&store, &settings))
                .unwrap();
        }
        store.active_account_id = Some(store.accounts[0].id.clone());
        menu.update(app, &tray::menu_entries(&store, &settings))
            .unwrap();
        let original_handle = menu.root.hpopupmenu().unwrap();
        let original_ids = command_ids(&menu.root);
        let allocation_count = menu.items.len();

        let test_tray = tauri::tray::TrayIconBuilder::with_id("native-menu-regression")
            .menu(&menu.root)
            .build(app)
            .unwrap();
        test_tray.set_visible(false).unwrap();
        let mut hwnd = HWND::default();
        unsafe {
            EnumThreadWindows(
                GetCurrentThreadId(),
                Some(find_test_tray),
                LPARAM(&mut hwnd as *mut HWND as isize),
            )
            .unwrap();
        }
        assert!(!hwnd.0.is_null());
        // Observe the real muda callback after WM_COMMAND, without executing any
        // production action (quit, account switching, or network requests).
        for iteration in 0..10_000 {
            store.active_account_id = Some(store.accounts[iteration % 10].id.clone());
            store.accounts[0].name = format!("Renamed {iteration}");
            settings.language = AppLanguage::new(if iteration % 2 == 0 { "en" } else { "zh-CN" });
            settings.floating.visible = iteration % 2 == 0;
            settings.taskbar.enabled = iteration % 3 == 0;
            let mut usage = UsageInfo::error(store.accounts[0].id.clone(), String::new());
            usage.error = None;
            usage.primary_used_percent = Some((iteration % 100) as f64);
            tray::TRAY_USAGE
                .lock()
                .unwrap()
                .insert(usage.account_id.clone(), usage);
            menu.update(app, &tray::menu_entries(&store, &settings))
                .unwrap();
        }
        assert_eq!(
            menu.items.len(),
            allocation_count,
            "polling must not allocate native menu IDs"
        );
        assert_eq!(menu.root.hpopupmenu().unwrap(), original_handle);
        store.active_account_id = Some(store.accounts[0].id.clone());
        menu.update(app, &tray::menu_entries(&store, &settings))
            .unwrap();
        // Account ordering changed, but each account keeps its own native ID.
        let ids = command_ids(&menu.root);
        for (key, id) in &ids {
            assert!(
                *id <= u16::MAX as u32,
                "{key} is not representable in WM_COMMAND"
            );
            if let Some(original) = original_ids.get(key) {
                assert_eq!(id, original);
            }
            unsafe {
                SendMessageW(hwnd, WM_COMMAND, WPARAM(*id as usize), LPARAM(0));
            }
            assert_eq!(
                rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
                *key
            );
        }
        // Empty / repopulated stores remove and reattach the SAME objects.
        menu.update(
            app,
            &tray::menu_entries(&AccountsStore::default(), &settings),
        )
        .unwrap();
        assert!(!command_ids(&menu.root).contains_key("account:native-menu-test-0"));
        menu.update(app, &tray::menu_entries(&store, &settings))
            .unwrap();
        assert_eq!(command_ids(&menu.root), ids);
        // Restore a checkbox toggled by the native selection even when settings
        // did not change (e.g. a failed save).
        let floating = menu.items[tray::FLOATING_VISIBLE_ID].as_check_menuitem_unchecked();
        floating.set_checked(!settings.floating.visible).unwrap();
        menu.update(app, &tray::menu_entries(&store, &settings))
            .unwrap();
        assert_eq!(
            menu.items[tray::FLOATING_VISIBLE_ID]
                .as_check_menuitem_unchecked()
                .is_checked()
                .unwrap(),
            settings.floating.visible
        );

        // Demonstrate the old failure using the locked muda version: allocations
        // advance its u32 counter beyond the 16-bit Windows command field.
        for _ in 0..66_000 {
            let _ = MenuItem::new(app, "legacy refresh", true, None::<&str>).unwrap();
        }
        let legacy = Menu::new(app).unwrap();
        legacy
            .append(&MenuItem::with_id(app, "legacy-open", "Open", true, None::<&str>).unwrap())
            .unwrap();
        let legacy_id = command_ids(&legacy)["legacy-open"];
        assert!(legacy_id > u16::MAX as u32);
        test_tray.set_menu(Some(legacy)).unwrap();
        unsafe {
            SendMessageW(
                hwnd,
                WM_COMMAND,
                WPARAM((legacy_id & 0xffff) as usize),
                LPARAM(0),
            );
        }
        assert!(
            rx.try_recv().is_err(),
            "the legacy overflow must reproduce the missing click"
        );
        test_tray.set_menu(Some(menu.root.clone())).unwrap();
        for key in [
            tray::OPEN_ITEM_ID,
            tray::SETTINGS_ITEM_ID,
            tray::QUIT_ITEM_ID,
        ] {
            unsafe {
                SendMessageW(hwnd, WM_COMMAND, WPARAM(ids[key] as usize), LPARAM(0));
            }
            assert_eq!(
                rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
                key
            );
        }
        eprintln!("10,000 updates: stable native IDs; legacy ID {legacy_id}: click lost; retained menu: clicks delivered");
        app.remove_tray_by_id("native-menu-regression");
    }
}
