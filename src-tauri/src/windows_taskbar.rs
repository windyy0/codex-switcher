//! Windows taskbar integration that keeps "close window" and "quit app"
//! as separate actions.

use std::thread;

use tauri::{AppHandle, Runtime};

use windows::{
    core::{Interface, PCWSTR, PROPVARIANT},
    Win32::{
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemAlloc, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED,
        },
        System::Variant::VT_LPWSTR,
        UI::Shell::{
            Common::{IObjectArray, IObjectCollection},
            DestinationList, EnumerableObjectCollection, ICustomDestinationList, IShellLinkW,
            PropertiesSystem::{IPropertyStore, PROPERTYKEY},
            ShellLink,
        },
    },
};

use crate::auth::load_app_settings;

pub const QUIT_ARGUMENT: &str = "--quit";
const APP_USER_MODEL_ID: &str = "com.lampese.codex-switcher";
const PKEY_TITLE: PROPERTYKEY = PROPERTYKEY {
    fmtid: windows::core::GUID::from_u128(0xf29f85e0_4ff9_1068_ab91_08002b27b3d9),
    pid: 2,
};

pub fn is_quit_request<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter()
        .any(|argument| argument.as_ref() == QUIT_ARGUMENT)
}

/// Handle a launch forwarded by the single-instance plugin.
///
/// On Windows the plugin invokes its callback synchronously from the hidden
/// `WM_COPYDATA` receiver window. Showing another window or exiting the app
/// from inside that window procedure is re-entrant: the call can appear to
/// succeed while the requested window state is never applied, and exiting can
/// tear down the receiver before the callback returns. Dispatch from a worker
/// so the action is queued for the next event-loop turn, after `WM_COPYDATA`
/// has completed.
pub fn handle_forwarded_launch<R: Runtime>(app: &AppHandle<R>, args: Vec<String>) {
    let should_quit = is_quit_request(&args);
    let app_handle = app.clone();
    if let Err(error) = thread::Builder::new()
        .name("taskbar-launch-dispatch".into())
        .spawn(move || {
            let action_app = app_handle.clone();
            if let Err(error) = app_handle.run_on_main_thread(move || {
                if should_quit {
                    action_app.exit(0);
                } else {
                    crate::commands::restore_main_window(&action_app);
                }
            }) {
                eprintln!("Failed to dispatch the taskbar command: {error}");
            }
        })
    {
        eprintln!("Failed to start the taskbar command dispatcher: {error}");
    }
}

/// Add an explicit quit command to the taskbar jump list. Windows' built-in
/// "Close window" command continues to hide the main window like the title-bar
/// close button; this separate task exits the application instead.
pub fn setup() {
    if let Err(error) = thread::Builder::new()
        .name("taskbar-jump-list".into())
        .spawn(|| {
            if let Err(error) = install_quit_task() {
                eprintln!("Failed to install the taskbar quit command: {error}");
            }
        })
    {
        eprintln!("Failed to start the taskbar jump-list worker: {error}");
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[repr(C)]
struct StringPropVariant {
    value_type: u16,
    reserved1: u16,
    reserved2: u16,
    reserved3: u16,
    value: *mut u16,
    padding: usize,
}

unsafe fn string_propvariant(value: &[u16]) -> windows::core::Result<PROPVARIANT> {
    debug_assert_eq!(
        std::mem::size_of::<StringPropVariant>(),
        std::mem::size_of::<PROPVARIANT>()
    );
    let bytes = std::mem::size_of_val(value);
    let allocation = CoTaskMemAlloc(bytes).cast::<u16>();
    if allocation.is_null() {
        return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
            0x8007000Eu32 as i32,
        )));
    }
    std::ptr::copy_nonoverlapping(value.as_ptr(), allocation, value.len());

    let raw = StringPropVariant {
        value_type: VT_LPWSTR.0,
        reserved1: 0,
        reserved2: 0,
        reserved3: 0,
        value: allocation,
        padding: 0,
    };
    Ok(std::mem::transmute(raw))
}

fn install_quit_task() -> windows::core::Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let result = install_quit_task_initialized();
        CoUninitialize();
        result
    }
}

unsafe fn install_quit_task_initialized() -> windows::core::Result<()> {
    let executable = std::env::current_exe().map_err(|error| {
        windows::core::Error::new(
            windows::core::HRESULT(0x80004005u32 as i32),
            format!("Could not resolve the current executable: {error}"),
        )
    })?;
    let executable = wide(&executable.to_string_lossy());
    let arguments = wide(QUIT_ARGUMENT);
    let app_id = wide(APP_USER_MODEL_ID);

    let settings = load_app_settings().unwrap_or_default();
    let language_code = crate::i18n::resolved_code(&settings.language);
    let title = wide(crate::i18n::text_for_code(language_code, "quitApp"));

    let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
    link.SetPath(PCWSTR(executable.as_ptr()))?;
    link.SetArguments(PCWSTR(arguments.as_ptr()))?;
    link.SetDescription(PCWSTR(title.as_ptr()))?;
    link.SetIconLocation(PCWSTR(executable.as_ptr()), 0)?;

    let properties: IPropertyStore = link.cast()?;
    let title_value = string_propvariant(&title)?;
    properties.SetValue(&PKEY_TITLE, &title_value)?;
    properties.Commit()?;

    let collection: IObjectCollection =
        CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER)?;
    collection.AddObject(&link)?;
    let tasks: IObjectArray = collection.cast()?;

    let destination_list: ICustomDestinationList =
        CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER)?;
    destination_list.SetAppID(PCWSTR(app_id.as_ptr()))?;
    let mut minimum_slots = 0;
    let _: IObjectArray = destination_list.BeginList(&mut minimum_slots)?;
    destination_list.AddUserTasks(&tasks)?;
    destination_list.CommitList()
}

#[cfg(test)]
mod tests {
    use super::{is_quit_request, string_propvariant, wide};

    #[test]
    fn recognizes_only_the_explicit_quit_argument() {
        assert!(is_quit_request(["codex-switcher.exe", "--quit"]));
        assert!(!is_quit_request(["codex-switcher.exe"]));
        assert!(!is_quit_request(["codex-switcher.exe", "--quit-now"]));
    }

    #[test]
    fn task_title_uses_a_property_system_string() {
        let title = wide("退出 Codex Switcher");
        let value = unsafe { string_propvariant(&title) }.expect("title should be allocated");
        let mut output = [0u16; 64];
        unsafe {
            windows::Win32::System::Com::StructuredStorage::PropVariantToString(
                &value,
                &mut output,
            )
            .expect("title should be readable by the property system");
        }
        let length = output.iter().position(|unit| *unit == 0).unwrap();
        assert_eq!(
            String::from_utf16_lossy(&output[..length]),
            "退出 Codex Switcher"
        );
    }
}
