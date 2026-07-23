//! Process detection commands

use std::process::Command;

#[cfg(any(windows, test))]
use anyhow::Context;

#[cfg(unix)]
use std::collections::HashMap;

#[cfg(any(unix, windows, test))]
use std::collections::HashSet;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use windows::{
    core::PWSTR,
    Win32::{
        Foundation::{CloseHandle, BOOL, ERROR_NO_MORE_FILES, FILETIME, HANDLE, HWND, LPARAM},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            },
            Threading::{
                GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
        UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, IsWindowVisible},
    },
};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;
#[cfg(any(windows, test))]
const WINDOWS_FILETIME_TICKS_PER_SECOND: u64 = 10_000_000;
#[cfg(any(windows, test))]
const WINDOWS_PROCESS_STARTUP_GRACE_SECONDS: u64 = 30;

#[cfg(any(windows, test))]
#[derive(Debug, Clone)]
struct WindowsProcessEntry {
    name: String,
    process_id: u32,
    parent_process_id: u32,
    executable_path: String,
    trusted_desktop_executable: bool,
    has_visible_window: bool,
    started_recently: bool,
}

#[cfg(windows)]
#[derive(Default)]
struct WindowsProcessDetails {
    executable_path: String,
    started_recently: bool,
}

#[cfg(windows)]
struct OwnedWindowsHandle(HANDLE);

#[cfg(windows)]
impl Drop for OwnedWindowsHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Information about running Codex processes
#[derive(Debug, Clone, serde::Serialize)]
pub struct CodexProcessInfo {
    /// Number of active Codex app instances
    pub count: usize,
    /// Number of ignored background/stale Codex-related processes
    pub background_count: usize,
    /// Whether switching is allowed (no active Codex app instances)
    pub can_switch: bool,
    /// Process IDs of active Codex app instances
    pub pids: Vec<u32>,
}

/// Summary of a force-close operation for active Codex processes.
#[derive(Debug, Clone, serde::Serialize)]
pub struct KillCodexProcessesResult {
    /// Number of active Codex sessions targeted before expanding child processes.
    pub targeted_count: usize,
    /// Process IDs that were successfully signalled for termination.
    pub killed_pids: Vec<u32>,
    /// Process IDs that could not be terminated.
    pub failed_pids: Vec<u32>,
}

#[cfg(unix)]
struct UnixProcessSnapshot {
    children_by_parent: HashMap<u32, Vec<u32>>,
    uid_by_pid: HashMap<u32, u32>,
}

const CODEX_RUNNING_SWITCH_BLOCKED_PREFIX: &str = "Cannot switch accounts while ";

/// Check for running Codex processes
#[tauri::command]
pub async fn check_codex_processes() -> Result<CodexProcessInfo, String> {
    let (pids, bg_count) = tokio::task::spawn_blocking(find_codex_processes)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let count = pids.len();

    Ok(CodexProcessInfo {
        count,
        background_count: bg_count,
        can_switch: count == 0,
        pids,
    })
}

pub(crate) fn ensure_codex_not_running() -> Result<(), String> {
    let (pids, _) = find_codex_processes().map_err(|e| e.to_string())?;

    if pids.is_empty() {
        return Ok(());
    }

    Err(format!(
        "{CODEX_RUNNING_SWITCH_BLOCKED_PREFIX}{} Codex process{} running",
        pids.len(),
        if pids.len() == 1 { " is" } else { "es are" }
    ))
}

pub(crate) fn is_codex_running_switch_block(error: &str) -> bool {
    error.starts_with(CODEX_RUNNING_SWITCH_BLOCKED_PREFIX)
}

/// Force-close active Codex processes that currently block account switching.
#[tauri::command]
pub async fn kill_codex_processes() -> Result<KillCodexProcessesResult, String> {
    tokio::task::spawn_blocking(kill_codex_processes_blocking)
        .await
        .map_err(|e| e.to_string())?
}

fn kill_codex_processes_blocking() -> Result<KillCodexProcessesResult, String> {
    let (pids, _) = find_codex_processes().map_err(|e| e.to_string())?;
    let targeted_count = pids.len();
    let mut killed_pids = Vec::new();
    let mut failed_pids = Vec::new();

    #[cfg(target_os = "macos")]
    let mut admin_targets: Vec<u32> = Vec::new();

    #[cfg(unix)]
    let snapshot = read_unix_process_snapshot();

    #[cfg(unix)]
    let targets = expand_process_targets(&pids, snapshot.as_ref());

    #[cfg(windows)]
    let targets = expand_process_targets(&pids);

    #[cfg(target_os = "macos")]
    let current_uid = current_unix_uid();

    for pid in targets {
        #[cfg(target_os = "macos")]
        if snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.uid_by_pid.get(&pid).copied())
            .zip(current_uid)
            .is_some_and(|(owner_uid, current_uid)| owner_uid != current_uid)
        {
            admin_targets.push(pid);
            continue;
        }

        if force_kill_process(pid) {
            killed_pids.push(pid);
        } else {
            failed_pids.push(pid);
        }
    }

    #[cfg(target_os = "macos")]
    {
        admin_targets.extend(failed_pids.iter().copied());
        admin_targets.sort_unstable();
        admin_targets.dedup();

        let mut still_failed = Vec::new();
        if force_kill_processes_with_admin_privileges(&admin_targets) {
            for pid in admin_targets.iter().copied() {
                if process_exists(pid) {
                    still_failed.push(pid);
                } else if !killed_pids.contains(&pid) {
                    killed_pids.push(pid);
                }
            }
        } else {
            still_failed.extend(
                admin_targets
                    .iter()
                    .copied()
                    .filter(|pid| process_exists(*pid)),
            );
        }
        failed_pids = still_failed;
    }

    Ok(KillCodexProcessesResult {
        targeted_count,
        killed_pids,
        failed_pids,
    })
}

#[cfg(unix)]
fn expand_process_targets(root_pids: &[u32], snapshot: Option<&UnixProcessSnapshot>) -> Vec<u32> {
    let mut targets = Vec::new();
    let mut visited = HashSet::new();

    if let Some(snapshot) = snapshot {
        for root_pid in root_pids {
            let mut stack = snapshot
                .children_by_parent
                .get(root_pid)
                .cloned()
                .unwrap_or_default();
            while let Some(pid) = stack.pop() {
                if !visited.insert(pid) {
                    continue;
                }
                targets.push(pid);

                if let Some(children) = snapshot.children_by_parent.get(&pid) {
                    stack.extend(children.iter().copied());
                }
            }
        }
    }

    for root_pid in root_pids {
        if visited.insert(*root_pid) {
            targets.push(*root_pid);
        }
    }

    targets
}

#[cfg(windows)]
fn expand_process_targets(root_pids: &[u32]) -> Vec<u32> {
    root_pids.to_vec()
}

#[cfg(unix)]
fn read_unix_process_snapshot() -> Option<UnixProcessSnapshot> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,ppid=,uid="])
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut children_by_parent = HashMap::new();
    let mut uid_by_pid = HashMap::new();

    for line in stdout.lines() {
        let mut parts = line.split_whitespace();
        let Some(pid_str) = parts.next() else {
            continue;
        };
        let Some(ppid_str) = parts.next() else {
            continue;
        };
        let Some(uid_str) = parts.next() else {
            continue;
        };
        let (Ok(pid), Ok(ppid), Ok(uid)) = (
            pid_str.parse::<u32>(),
            ppid_str.parse::<u32>(),
            uid_str.parse::<u32>(),
        ) else {
            continue;
        };

        children_by_parent
            .entry(ppid)
            .or_insert_with(Vec::new)
            .push(pid);
        uid_by_pid.insert(pid, uid);
    }

    Some(UnixProcessSnapshot {
        children_by_parent,
        uid_by_pid,
    })
}

fn force_kill_process(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let killed = Command::new("/bin/kill")
            .arg("-9")
            .arg(pid.to_string())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        return killed || !process_exists(pid);
    }

    #[cfg(windows)]
    {
        let killed = Command::new("taskkill")
            .creation_flags(CREATE_NO_WINDOW)
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        return killed || !process_exists(pid);
    }

    #[allow(unreachable_code)]
    false
}

#[cfg(target_os = "macos")]
fn force_kill_processes_with_admin_privileges(pids: &[u32]) -> bool {
    if pids.is_empty() {
        return true;
    }

    let pid_args = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    let script = format!(
        r#"do shell script "for pid in {pid_args}; do /bin/kill -9 \"$pid\" 2>/dev/null || true; done" with administrator privileges with prompt "Codex Switcher needs permission to force close sudo/root Codex processes.""#
    );

    Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn current_unix_uid() -> Option<u32> {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .parse::<u32>()
                    .ok()
            } else {
                None
            }
        })
}

fn process_exists(pid: u32) -> bool {
    #[cfg(unix)]
    {
        return Command::new("ps")
            .arg("-p")
            .arg(pid.to_string())
            .args(["-o", "pid="])
            .output()
            .map(|output| {
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout)
                        .split_whitespace()
                        .any(|value| value == pid.to_string())
            })
            .unwrap_or(false);
    }

    #[cfg(windows)]
    {
        return Command::new("tasklist")
            .creation_flags(CREATE_NO_WINDOW)
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
            .unwrap_or(false);
    }

    #[allow(unreachable_code)]
    false
}

/// Find all running codex processes. Returns (active_pids, background_count)
fn find_codex_processes() -> anyhow::Result<(Vec<u32>, usize)> {
    #[cfg(unix)]
    {
        let mut pids = Vec::new();
        let mut bg_count = 0;
        let process_names = read_unix_process_names();

        // Include TTY so we can distinguish interactive CLI sessions from
        // background helper processes such as lingering app-server instances.
        let output = Command::new("ps")
            .args(["-axo", "pid=,tty=,command="])
            .output();

        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                let mut parts = line.split_whitespace();
                let Some(pid_str) = parts.next() else {
                    continue;
                };
                let Some(tty) = parts.next() else {
                    continue;
                };
                let command = parts.collect::<Vec<_>>().join(" ");
                if command.is_empty() {
                    continue;
                }

                let Ok(pid) = pid_str.parse::<u32>() else {
                    continue;
                };

                let lowercase_command = command.to_ascii_lowercase();
                let is_switcher = lowercase_command.contains("codex-switcher");

                if is_switcher {
                    continue;
                }

                // macOS app bundle paths can contain spaces (`Codex Helper.app`), so
                // splitting on whitespace can turn helper processes into false
                // positives for the main `Codex` app. Detect by full command shape
                // instead of relying on the first token.
                let first_token = command.split_whitespace().next().unwrap_or("");
                let is_codex_cli = first_token == "codex" || first_token.ends_with("/codex");
                let process_name = process_names.get(&pid).map(String::as_str);
                #[cfg(target_os = "macos")]
                let bundle_identifier = read_macos_app_bundle_identifier(&command, process_name);
                #[cfg(not(target_os = "macos"))]
                let bundle_identifier: Option<String> = None;
                let is_codex_desktop = is_macos_codex_desktop_process(
                    &command,
                    process_name,
                    bundle_identifier.as_deref(),
                );

                if !is_codex_cli && !is_codex_desktop {
                    continue;
                }

                if pid == std::process::id() || pids.contains(&pid) {
                    continue;
                }

                let is_ide_plugin = is_ide_plugin_process(&lowercase_command);
                let is_app_server = lowercase_command.contains("codex app-server");
                let has_tty = tty != "??" && tty != "?";

                if is_ide_plugin || is_app_server {
                    bg_count += 1;
                    continue;
                }

                if is_codex_desktop || has_tty {
                    pids.push(pid);
                } else {
                    // Headless or orphaned codex processes should not block switching.
                    bg_count += 1;
                }
            }
        }

        pids.sort_unstable();
        pids.dedup();

        return Ok((pids, bg_count));
    }

    #[cfg(windows)]
    {
        return find_windows_codex_processes();
    }

    #[allow(unreachable_code)]
    Ok((Vec::new(), 0))
}

#[cfg(unix)]
fn read_unix_process_names() -> HashMap<u32, String> {
    let Ok(output) = Command::new("ps").args(["-axo", "pid=,ucomm="]).output() else {
        return HashMap::new();
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.trim().splitn(2, char::is_whitespace);
            let pid = parts.next()?.parse::<u32>().ok()?;
            let name = parts.next()?.trim();
            (!name.is_empty()).then(|| (pid, name.to_string()))
        })
        .collect()
}

#[cfg(unix)]
fn is_macos_codex_desktop_process(
    command: &str,
    process_name: Option<&str>,
    bundle_identifier: Option<&str>,
) -> bool {
    #[cfg(not(target_os = "macos"))]
    let _ = bundle_identifier;

    const LEGACY_EXECUTABLE_SUFFIX: &str = "/Codex.app/Contents/MacOS/Codex";
    #[cfg(target_os = "macos")]
    const CURRENT_EXECUTABLE_SUFFIX: &str = "/ChatGPT.app/Contents/MacOS/ChatGPT";
    #[cfg(target_os = "macos")]
    const CODEX_BUNDLE_IDENTIFIER: &str = "com.openai.codex";

    let executable_suffix = match process_name {
        Some("Codex") => LEGACY_EXECUTABLE_SUFFIX,
        #[cfg(target_os = "macos")]
        Some("ChatGPT") if bundle_identifier == Some(CODEX_BUNDLE_IDENTIFIER) => {
            CURRENT_EXECUTABLE_SUFFIX
        }
        _ => return false,
    };

    command.find(executable_suffix).is_some_and(|index| {
        command[index + executable_suffix.len()..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace)
    })
}

#[cfg(target_os = "macos")]
fn read_macos_app_bundle_identifier(command: &str, process_name: Option<&str>) -> Option<String> {
    const APP_BUNDLE_SUFFIX: &str = "/ChatGPT.app";
    const EXECUTABLE_SUFFIX: &str = "/ChatGPT.app/Contents/MacOS/ChatGPT";

    if process_name != Some("ChatGPT") {
        return None;
    }

    let executable_index = command.find(EXECUTABLE_SUFFIX)?;
    let executable_end = executable_index + EXECUTABLE_SUFFIX.len();
    if command[executable_end..]
        .chars()
        .next()
        .is_some_and(|character| !character.is_whitespace())
    {
        return None;
    }

    let bundle_end = executable_index + APP_BUNDLE_SUFFIX.len();
    let info_plist = std::path::Path::new(&command[..bundle_end]).join("Contents/Info.plist");
    let value = plist::Value::from_file(info_plist).ok()?;

    value
        .as_dictionary()?
        .get("CFBundleIdentifier")?
        .as_string()
        .map(str::to_owned)
}

#[cfg(windows)]
fn find_windows_codex_processes() -> anyhow::Result<(Vec<u32>, usize)> {
    // Toolhelp and User32 are local, bounded Win32 calls. Keeping the process snapshot native
    // avoids both the WMI/CIM stalls seen on some machines and stdout-pipe backpressure from a
    // helper PowerShell process. Parent PIDs are preserved so startup-time renderer/app-server
    // trees still block account switching before the main window has a title.
    let processes = read_windows_process_snapshot()?;
    Ok(classify_windows_codex_processes(&processes))
}

#[cfg(windows)]
fn read_windows_process_snapshot() -> anyhow::Result<Vec<WindowsProcessEntry>> {
    let window_processes = read_windows_window_processes()?;
    let snapshot = OwnedWindowsHandle(
        unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
            .context("failed to snapshot Windows processes")?,
    );
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    unsafe { Process32FirstW(snapshot.0, &mut entry) }
        .context("failed to read the first Windows process")?;

    let mut processes = Vec::new();
    loop {
        let name = utf16_c_string(&entry.szExeFile);
        let process_id = entry.th32ProcessID;
        let needs_path =
            name.eq_ignore_ascii_case("Codex.exe") || name.eq_ignore_ascii_case("ChatGPT.exe");
        let details = if needs_path {
            read_windows_process_details(process_id)
                .with_context(|| format!("failed to inspect {name} process (PID {process_id})"))?
        } else {
            WindowsProcessDetails::default()
        };
        let trusted_desktop_executable = if name.eq_ignore_ascii_case("Codex.exe") {
            is_windows_legacy_codex_desktop_path(&details.executable_path)
                || is_windows_codex_package_root_path(&details.executable_path)
        } else if name.eq_ignore_ascii_case("ChatGPT.exe") {
            is_windows_codex_package_root_path(&details.executable_path)
        } else {
            false
        };
        processes.push(WindowsProcessEntry {
            name,
            process_id,
            parent_process_id: entry.th32ParentProcessID,
            executable_path: details.executable_path,
            trusted_desktop_executable,
            has_visible_window: window_processes.contains(&process_id),
            started_recently: details.started_recently,
        });

        match unsafe { Process32NextW(snapshot.0, &mut entry) } {
            Ok(()) => {}
            Err(error) if error.code() == ERROR_NO_MORE_FILES.to_hresult() => break,
            Err(error) => return Err(error).context("failed to read the next Windows process"),
        }
    }

    Ok(processes)
}

#[cfg(windows)]
fn read_windows_process_details(process_id: u32) -> anyhow::Result<WindowsProcessDetails> {
    let process = OwnedWindowsHandle(
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }
            .context("failed to open Windows process")?,
    );

    Ok(WindowsProcessDetails {
        executable_path: read_windows_process_path(process.0)?,
        started_recently: windows_process_started_recently(process.0)?,
    })
}

#[cfg(windows)]
fn read_windows_process_path(process: HANDLE) -> anyhow::Result<String> {
    let mut path = vec![0u16; 32_768];
    let mut path_len = path.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(path.as_mut_ptr()),
            &mut path_len,
        )
    }
    .context("failed to read Windows process path")?;
    path.truncate(path_len as usize);
    Ok(String::from_utf16_lossy(&path))
}

#[cfg(windows)]
fn windows_process_started_recently(process: HANDLE) -> anyhow::Result<bool> {
    const FILETIME_UNIX_EPOCH_OFFSET_SECONDS: u64 = 11_644_473_600;

    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) }
        .context("failed to read Windows process times")?;

    let created_ticks =
        (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    let since_unix_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?;
    let now_ticks = since_unix_epoch
        .as_secs()
        .saturating_add(FILETIME_UNIX_EPOCH_OFFSET_SECONDS)
        .saturating_mul(WINDOWS_FILETIME_TICKS_PER_SECOND)
        .saturating_add(u64::from(since_unix_epoch.subsec_nanos()) / 100);

    Ok(is_recent_windows_process_start(created_ticks, now_ticks))
}

#[cfg(any(windows, test))]
fn is_recent_windows_process_start(created_ticks: u64, now_ticks: u64) -> bool {
    created_ticks != 0
        && created_ticks <= now_ticks
        && now_ticks - created_ticks
            <= WINDOWS_PROCESS_STARTUP_GRACE_SECONDS
                .saturating_mul(WINDOWS_FILETIME_TICKS_PER_SECOND)
}

#[cfg(windows)]
fn read_windows_window_processes() -> anyhow::Result<HashSet<u32>> {
    let mut process_ids = HashSet::new();
    unsafe {
        EnumWindows(
            Some(collect_windows_window_process),
            LPARAM((&mut process_ids as *mut HashSet<u32>) as isize),
        )
    }
    .context("failed to enumerate Windows top-level windows")?;
    Ok(process_ids)
}

#[cfg(windows)]
unsafe extern "system" fn collect_windows_window_process(hwnd: HWND, state: LPARAM) -> BOOL {
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return true.into();
    }

    let mut process_id = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process_id)) };
    if process_id != 0 {
        let process_ids = unsafe { &mut *(state.0 as *mut HashSet<u32>) };
        process_ids.insert(process_id);
    }
    true.into()
}

#[cfg(any(windows, test))]
fn utf16_c_string(value: &[u16]) -> String {
    let end = value
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(value.len());
    String::from_utf16_lossy(&value[..end])
}

#[cfg(any(windows, test))]
fn classify_windows_codex_processes(processes: &[WindowsProcessEntry]) -> (Vec<u32>, usize) {
    let mut active_pids = Vec::new();
    let mut ignored_count = 0;

    for process in processes
        .iter()
        .filter(|process| is_windows_codex_root_process(process, processes))
    {
        let has_window = process.has_visible_window
            || windows_has_descendant_matching(process.process_id, processes, |child| {
                child.has_visible_window
            });
        let has_app_server =
            windows_has_descendant_matching(process.process_id, processes, |child| {
                normalize_windows_path(&child.executable_path).contains("\\resources\\codex.exe")
            });

        if has_window || process.started_recently || has_app_server {
            active_pids.push(process.process_id);
        } else {
            // Ignore a lone headless root and stale Electron helpers after the desktop window
            // and app-server have exited.
            ignored_count += 1;
        }
    }

    active_pids.sort_unstable();
    active_pids.dedup();

    (active_pids, ignored_count)
}

#[cfg(any(windows, test))]
fn is_windows_codex_root_process(
    process: &WindowsProcessEntry,
    processes: &[WindowsProcessEntry],
) -> bool {
    if !is_windows_codex_candidate(process) {
        return false;
    }

    !windows_has_candidate_ancestor(process.parent_process_id, processes)
}

#[cfg(any(windows, test))]
fn is_windows_codex_candidate(process: &WindowsProcessEntry) -> bool {
    let name = process.name.to_ascii_lowercase();
    let executable_path = normalize_windows_path(&process.executable_path);
    if executable_path.contains("codex-switcher") || is_ide_plugin_process(&executable_path) {
        return false;
    }

    if name == "codex.exe" {
        // These PIDs are also offered to the force-close path, so an arbitrary
        // same-name executable must never be treated as the desktop app. A
        // failed path inspection aborts the Windows snapshot before reaching
        // this classifier.
        return process.trusted_desktop_executable;
    }

    name == "chatgpt.exe" && process.trusted_desktop_executable
}

#[cfg(windows)]
fn is_windows_legacy_codex_desktop_path(path: &str) -> bool {
    let local_match = std::env::var("LOCALAPPDATA").ok().is_some_and(|root| {
        windows_path_relative_to_root(path, &root)
            .is_some_and(|relative| is_supported_local_app_data_codex_path(&relative))
    });
    local_match
        || ["ProgramFiles", "ProgramFiles(x86)"]
            .iter()
            .filter_map(|key| std::env::var(key).ok())
            .any(|root| {
                windows_path_relative_to_root(path, &root)
                    .is_some_and(|relative| is_supported_program_files_codex_path(&relative))
            })
}

#[cfg(any(windows, test))]
fn windows_path_relative_to_root(path: &str, root: &str) -> Option<String> {
    let path = normalize_windows_absolute_path(path.trim().trim_matches('"'));
    let root = normalize_windows_absolute_path(root.trim().trim_matches('"'))
        .trim_end_matches('\\')
        .to_string();
    path.strip_prefix(&format!("{root}\\")).map(str::to_owned)
}

#[cfg(any(windows, test))]
fn is_supported_local_app_data_codex_path(relative_path: &str) -> bool {
    let relative = normalize_windows_path(relative_path)
        .trim_matches('\\')
        .to_string();
    if [
        "codex\\codex.exe",
        "openai\\codex\\codex.exe",
        "openai codex\\codex.exe",
        "codex desktop\\codex.exe",
    ]
    .contains(&relative.as_str())
    {
        return true;
    }

    let programs_layout = [
        "programs\\codex\\codex.exe",
        "programs\\openai\\codex\\codex.exe",
        "programs\\openai codex\\codex.exe",
        "programs\\codex desktop\\codex.exe",
    ]
    .contains(&relative.as_str());
    programs_layout
}

#[cfg(any(windows, test))]
fn is_supported_program_files_codex_path(relative_path: &str) -> bool {
    let relative = normalize_windows_path(relative_path)
        .trim_matches('\\')
        .to_string();
    [
        "programs\\codex\\codex.exe",
        "programs\\openai\\codex\\codex.exe",
        "programs\\openai codex\\codex.exe",
        "programs\\codex desktop\\codex.exe",
        "codex\\codex.exe",
        "openai\\codex\\codex.exe",
        "openai codex\\codex.exe",
        "codex desktop\\codex.exe",
    ]
    .contains(&relative.as_str())
}

#[cfg(any(windows, test))]
fn windows_has_candidate_ancestor(
    parent_process_id: u32,
    processes: &[WindowsProcessEntry],
) -> bool {
    let mut current_pid = parent_process_id;
    let mut visited = HashSet::new();
    while current_pid != 0 && visited.insert(current_pid) {
        let Some(parent) = processes
            .iter()
            .find(|process| process.process_id == current_pid)
        else {
            break;
        };
        if is_windows_codex_candidate(parent) {
            return true;
        }
        current_pid = parent.parent_process_id;
    }
    false
}

#[cfg(any(windows, test))]
fn is_windows_codex_package_root_path(path: &str) -> bool {
    let normalized = normalize_windows_absolute_path(path.trim().trim_matches('"'));
    let Some(package_path) = normalized
        .strip_suffix("\\app\\chatgpt.exe")
        .or_else(|| normalized.strip_suffix("\\app\\codex.exe"))
    else {
        return false;
    };
    let mut components = package_path.rsplit('\\');
    let Some(package_name) = components.next() else {
        return false;
    };
    let windows_apps_path = components.collect::<Vec<_>>();
    let trusted_windows_apps_root = matches!(
        windows_apps_path.as_slice(),
        ["windowsapps", drive] if drive.ends_with(':')
    ) || matches!(
        windows_apps_path.as_slice(),
        ["windowsapps", "program files", drive] if drive.ends_with(':')
    );

    trusted_windows_apps_root
        && package_name.starts_with("openai.codex_")
        && package_name.ends_with("__2p2nqsd0c76g0")
}

#[cfg(any(windows, test))]
fn normalize_windows_path(value: &str) -> String {
    value.replace('/', "\\").to_ascii_lowercase()
}

#[cfg(any(windows, test))]
fn normalize_windows_absolute_path(value: &str) -> String {
    let normalized = normalize_windows_path(value);
    normalized
        .strip_prefix("\\\\?\\")
        .unwrap_or(&normalized)
        .to_string()
}

#[cfg(any(unix, windows, test))]
fn is_ide_plugin_process(command: &str) -> bool {
    command.contains(".antigravity")
        || command.contains("openai.chatgpt")
        || command.contains(".vscode")
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::is_macos_codex_desktop_process;
    use super::{
        classify_windows_codex_processes, is_recent_windows_process_start,
        is_supported_local_app_data_codex_path, is_supported_program_files_codex_path,
        is_windows_codex_candidate, is_windows_codex_root_process, normalize_windows_path,
        utf16_c_string, windows_path_relative_to_root, WindowsProcessEntry,
    };

    fn windows_process(
        name: &str,
        process_id: u32,
        parent_process_id: u32,
        executable_path: &str,
        has_visible_window: bool,
    ) -> WindowsProcessEntry {
        let normalized_path = normalize_windows_path(executable_path);
        let trusted_desktop_executable = if name.eq_ignore_ascii_case("Codex.exe") {
            windows_path_relative_to_root(&normalized_path, r"C:\Users\test\AppData\Local")
                .is_some_and(|relative| is_supported_local_app_data_codex_path(&relative))
                || windows_path_relative_to_root(&normalized_path, r"C:\Program Files")
                    .is_some_and(|relative| is_supported_program_files_codex_path(&relative))
                || windows_path_relative_to_root(&normalized_path, r"C:\Program Files (x86)")
                    .is_some_and(|relative| is_supported_program_files_codex_path(&relative))
                || super::is_windows_codex_package_root_path(&normalized_path)
        } else if name.eq_ignore_ascii_case("ChatGPT.exe") {
            super::is_windows_codex_package_root_path(&normalized_path)
        } else {
            false
        };
        WindowsProcessEntry {
            name: name.to_string(),
            process_id,
            parent_process_id,
            executable_path: executable_path.to_string(),
            trusted_desktop_executable,
            has_visible_window,
            started_recently: false,
        }
    }

    #[cfg(unix)]
    #[test]
    fn detects_only_the_legacy_macos_codex_desktop_root_process() {
        assert!(is_macos_codex_desktop_process(
            "/Applications/Codex.app/Contents/MacOS/Codex",
            Some("Codex"),
            None
        ));
        assert!(is_macos_codex_desktop_process(
            "/Users/test/Applications With Spaces/Codex.app/Contents/MacOS/Codex --flag",
            Some("Codex"),
            None
        ));
        assert!(!is_macos_codex_desktop_process(
            "/Applications/Codex.app/Contents/Frameworks/Codex Framework.framework/Helpers/Codex (Service).app/Contents/MacOS/Codex (Service) --type=gpu-process",
            Some("Codex (Service)"),
            None
        ));
        assert!(!is_macos_codex_desktop_process(
            "/Applications/Codex.app/Contents/Frameworks/Codex Framework.framework/Helpers/Codex (Renderer).app/Contents/MacOS/Codex (Renderer) --type=renderer",
            Some("Codex (Renderer)"),
            None
        ));
        assert!(!is_macos_codex_desktop_process(
            "/Applications/Codex.app/Contents/Resources/codex app-server",
            Some("codex"),
            None
        ));
        assert!(!is_macos_codex_desktop_process(
            "/Applications/Codex.app/Contents/Frameworks/Codex Framework.framework/Helpers/Codex (Renderer).app/Contents/MacOS/Codex (Renderer) --app-executable /Applications/Codex.app/Contents/MacOS/Codex --type=renderer",
            Some("Codex (Renderer)"),
            None
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn detects_only_the_current_macos_codex_desktop_root_process() {
        assert!(is_macos_codex_desktop_process(
            "/Applications/ChatGPT.app/Contents/MacOS/ChatGPT",
            Some("ChatGPT"),
            Some("com.openai.codex")
        ));
        assert!(is_macos_codex_desktop_process(
            "/Users/test/Applications With Spaces/ChatGPT.app/Contents/MacOS/ChatGPT --flag",
            Some("ChatGPT"),
            Some("com.openai.codex")
        ));
        assert!(!is_macos_codex_desktop_process(
            "/Applications/ChatGPT.app/Contents/MacOS/ChatGPT",
            Some("ChatGPT"),
            Some("com.openai.chat")
        ));
        assert!(!is_macos_codex_desktop_process(
            "/Applications/ChatGPT.app/Contents/MacOS/ChatGPT",
            Some("ChatGPT"),
            None
        ));
        assert!(!is_macos_codex_desktop_process(
            "/Applications/ChatGPT.app/Contents/MacOS/ChatGPT",
            Some("Codex"),
            Some("com.openai.codex")
        ));
        assert!(!is_macos_codex_desktop_process(
            "/Applications/ChatGPT.app/Contents/Frameworks/Codex Framework.framework/Helpers/Codex (Renderer).app/Contents/MacOS/Codex (Renderer) --app-executable /Applications/ChatGPT.app/Contents/MacOS/ChatGPT --type=renderer",
            Some("Codex (Renderer)"),
            Some("com.openai.codex")
        ));
    }

    #[test]
    fn decodes_null_terminated_toolhelp_names() {
        assert_eq!(utf16_c_string(&[67, 111, 100, 101, 120, 0, 88]), "Codex");
        assert_eq!(utf16_c_string(&[67, 111, 100, 101, 120]), "Codex");
    }

    #[test]
    fn windows_startup_grace_period_has_safe_time_boundaries() {
        const SECOND: u64 = super::WINDOWS_FILETIME_TICKS_PER_SECOND;
        let now = 1_000 * SECOND;

        assert!(is_recent_windows_process_start(now, now));
        assert!(is_recent_windows_process_start(now - 30 * SECOND, now));
        assert!(!is_recent_windows_process_start(now - 31 * SECOND, now));
        assert!(!is_recent_windows_process_start(now + SECOND, now));
        assert!(!is_recent_windows_process_start(0, now));
    }

    #[test]
    fn windows_root_detection_supports_legacy_and_current_packages() {
        let legacy_root = windows_process(
            "Codex.exe",
            10,
            1,
            r"C:\Users\test\AppData\Local\Programs\Codex\Codex.exe",
            true,
        );
        let current_root = windows_process(
            "ChatGPT.exe",
            20,
            1,
            r"C:\Program Files\WindowsApps\OpenAI.Codex_26.707.3748.0_x64__2p2nqsd0c76g0\app\ChatGPT.exe",
            true,
        );
        let legacy_packaged_root = windows_process(
            "Codex.exe",
            21,
            1,
            r"C:\Program Files\WindowsApps\OpenAI.Codex_26.429.3425.0_x64__2p2nqsd0c76g0\app\Codex.exe",
            true,
        );

        assert!(is_windows_codex_root_process(
            &legacy_root,
            std::slice::from_ref(&legacy_root)
        ));
        assert!(is_windows_codex_root_process(
            &current_root,
            std::slice::from_ref(&current_root)
        ));
        assert!(is_windows_codex_root_process(
            &legacy_packaged_root,
            std::slice::from_ref(&legacy_packaged_root)
        ));
    }

    #[test]
    fn windows_root_detection_rejects_helpers_backends_and_unrelated_chatgpt() {
        let bundled_backend = windows_process(
            "Codex.exe",
            30,
            20,
            r"C:\Program Files\WindowsApps\OpenAI.Codex_26.707.3748.0_x64__2p2nqsd0c76g0\app\resources\codex.exe",
            false,
        );
        let unrelated_chatgpt = windows_process(
            "ChatGPT.exe",
            32,
            1,
            r"C:\Program Files\WindowsApps\OpenAI.ChatGPT_26.707.3748.0_x64__2p2nqsd0c76g0\app\ChatGPT.exe",
            true,
        );
        let wrong_publisher = windows_process(
            "ChatGPT.exe",
            33,
            1,
            r"C:\Program Files\WindowsApps\OpenAI.Codex_26.707.3748.0_x64__notcodex\app\ChatGPT.exe",
            true,
        );
        let lookalike_outside_windows_apps = windows_process(
            "ChatGPT.exe",
            34,
            1,
            r"C:\Temp\OpenAI.Codex_26.707.3748.0_x64__2p2nqsd0c76g0\app\ChatGPT.exe",
            true,
        );
        let ide_helper = windows_process(
            "Codex.exe",
            35,
            1,
            r"C:\Users\test\.vscode\extensions\openai.chatgpt\bin\codex.exe",
            false,
        );
        let unrelated_codex =
            windows_process("Codex.exe", 36, 1, r"C:\Tools\Codex\Codex.exe", true);
        let forged_legacy_suffix = windows_process(
            "Codex.exe",
            37,
            1,
            r"D:\Fake\AppData\Local\Programs\Codex\Codex.exe",
            true,
        );
        let forged_store_suffix = windows_process(
            "ChatGPT.exe",
            38,
            1,
            r"D:\Fake\WindowsApps\OpenAI.Codex_26.707.3748.0_x64__2p2nqsd0c76g0\app\ChatGPT.exe",
            true,
        );

        for process in [
            bundled_backend,
            unrelated_chatgpt,
            wrong_publisher,
            lookalike_outside_windows_apps,
            ide_helper,
            unrelated_codex,
            forged_legacy_suffix,
            forged_store_suffix,
        ] {
            assert!(!is_windows_codex_candidate(&process));
        }
    }

    #[test]
    fn trusted_windows_install_layouts_cover_supported_fallbacks() {
        for path in [
            r"C:\Users\test\AppData\Local\Codex\Codex.exe",
            r"C:\Users\test\AppData\Local\OpenAI\Codex\Codex.exe",
            r"C:\Users\test\AppData\Local\OpenAI Codex\Codex.exe",
            r"C:\Users\test\AppData\Local\Codex Desktop\Codex.exe",
            r"C:\Users\test\AppData\Local\Programs\OpenAI Codex\Codex.exe",
            r"C:\Program Files\Codex\Codex.exe",
            r"C:\Program Files (x86)\OpenAI\Codex\Codex.exe",
        ] {
            assert!(is_windows_codex_candidate(&windows_process(
                "Codex.exe",
                50,
                1,
                path,
                false,
            )));
        }

        assert!(is_windows_codex_candidate(&windows_process(
            "ChatGPT.exe",
            51,
            1,
            r"D:\WindowsApps\OpenAI.Codex_26.707.3748.0_x64__2p2nqsd0c76g0\app\ChatGPT.exe",
            false,
        )));
        for path in [
            r"C:\Users\test\AppData\Local\OpenAI\Codex\bin\codex.exe",
            r"C:\Users\test\AppData\Local\Programs\OpenAI\Codex\bin\codex.exe",
            r"C:\Users\test\AppData\Local\Packages\OpenAI.Codex_26.707.3748.0_x64__2p2nqsd0c76g0\LocalCache\Local\OpenAI\Codex\bin\codex.exe",
        ] {
            assert!(!is_windows_codex_candidate(&windows_process(
                "Codex.exe",
                52,
                1,
                path,
                true,
            )));
        }
    }

    #[test]
    fn classifies_recent_legacy_startup_and_current_app_server() {
        let mut legacy_startup = windows_process(
            "Codex.exe",
            100,
            1,
            r"C:\Users\test\AppData\Local\Programs\Codex\Codex.exe",
            false,
        );
        legacy_startup.started_recently = true;
        let processes = vec![
            legacy_startup,
            windows_process(
                "ChatGPT.exe",
                200,
                1,
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.707.3748.0_x64__2p2nqsd0c76g0\app\ChatGPT.exe",
                false,
            ),
            windows_process(
                "Codex.exe",
                201,
                200,
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.707.3748.0_x64__2p2nqsd0c76g0\app\resources\codex.exe",
                false,
            ),
        ];

        assert_eq!(
            classify_windows_codex_processes(&processes),
            (vec![100, 200], 0)
        );
    }

    #[test]
    fn helper_descendants_are_not_reported_as_additional_roots() {
        let processes = vec![
            windows_process(
                "ChatGPT.exe",
                200,
                1,
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.707.3748.0_x64__2p2nqsd0c76g0\app\ChatGPT.exe",
                true,
            ),
            windows_process(
                "Codex.exe",
                201,
                200,
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.707.3748.0_x64__2p2nqsd0c76g0\app\resources\codex.exe",
                false,
            ),
            windows_process(
                "Codex.exe",
                202,
                201,
                r"C:\Users\test\AppData\Local\OpenAI\Codex\bin\build\codex.exe",
                false,
            ),
        ];

        assert!(!is_windows_codex_root_process(&processes[2], &processes));
        assert_eq!(classify_windows_codex_processes(&processes), (vec![200], 0));
    }

    #[test]
    fn ignores_stale_roots_even_when_generic_helpers_remain() {
        let processes = vec![
            windows_process(
                "Codex.exe",
                100,
                1,
                r"C:\Users\test\AppData\Local\Programs\Codex\Codex.exe",
                false,
            ),
            windows_process(
                "Codex.exe",
                101,
                100,
                r"C:\Users\test\AppData\Local\Programs\Codex\Codex.exe",
                false,
            ),
            windows_process(
                "ChatGPT.exe",
                200,
                1,
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.707.3748.0_x64__2p2nqsd0c76g0\app\ChatGPT.exe",
                false,
            ),
        ];

        assert_eq!(classify_windows_codex_processes(&processes), (vec![], 2));
    }

    #[test]
    fn follows_process_trees_through_non_codex_intermediates() {
        let processes = vec![
            windows_process(
                "ChatGPT.exe",
                200,
                1,
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.707.3748.0_x64__2p2nqsd0c76g0\app\ChatGPT.exe",
                false,
            ),
            windows_process("RuntimeBroker.exe", 201, 200, "", false),
            windows_process(
                "Codex.exe",
                202,
                201,
                r"C:\Program Files\WindowsApps\OpenAI.Codex_26.707.3748.0_x64__2p2nqsd0c76g0\app\resources\codex.exe",
                false,
            ),
            windows_process(
                "Codex.exe",
                203,
                201,
                r"C:\Users\test\AppData\Local\OpenAI\Codex\bin\codex.exe",
                false,
            ),
        ];

        assert!(!is_windows_codex_root_process(&processes[3], &processes));
        assert_eq!(classify_windows_codex_processes(&processes), (vec![200], 0));
    }

    #[test]
    fn refuses_to_force_close_codex_processes_without_a_trusted_path() {
        let process = windows_process("Codex.exe", 100, 1, "", true);
        assert!(!is_windows_codex_candidate(&process));
    }

    #[test]
    fn process_tree_cycles_do_not_loop_or_create_extra_roots() {
        let processes = vec![
            windows_process(
                "Codex.exe",
                100,
                101,
                r"C:\Users\test\AppData\Local\Programs\Codex\Codex.exe",
                false,
            ),
            windows_process("RuntimeBroker.exe", 101, 100, "", false),
        ];

        assert_eq!(classify_windows_codex_processes(&processes), (vec![], 0));
    }

    #[test]
    fn windows_codex_shortcut_filter_excludes_switcher() {
        assert!(!super::is_windows_codex_shortcut_name("ChatGPT.lnk"));
        assert!(!super::is_windows_codex_shortcut_name("OpenAI ChatGPT.lnk"));
        assert!(super::is_windows_codex_shortcut_name("Codex.lnk"));
        assert!(super::is_windows_codex_shortcut_name("OpenAI Codex.lnk"));
        assert!(!super::is_windows_codex_shortcut_name("Codex Switcher.lnk"));
        assert!(!super::is_windows_codex_shortcut_name("codex-switcher.lnk"));
        assert!(!super::is_windows_codex_shortcut_name("Codex.txt"));
    }

    #[cfg(windows)]
    #[test]
    fn native_windows_snapshot_is_bounded_and_well_formed() {
        let processes = super::read_windows_process_snapshot()
            .expect("native Windows process snapshot should succeed");
        let mut pids = std::collections::HashSet::new();
        assert!(!processes.is_empty());
        assert!(processes
            .iter()
            .all(|process| !process.name.is_empty() && pids.insert(process.process_id)));
    }
}

#[cfg(any(windows, test))]
fn windows_has_descendant_matching<F>(
    root_pid: u32,
    processes: &[WindowsProcessEntry],
    mut predicate: F,
) -> bool
where
    F: FnMut(&WindowsProcessEntry) -> bool,
{
    let mut queue = vec![root_pid];
    let mut visited = HashSet::new();

    while let Some(parent_pid) = queue.pop() {
        for process in processes
            .iter()
            .filter(|process| process.parent_process_id == parent_pid)
        {
            if !visited.insert(process.process_id) {
                continue;
            }

            if predicate(process) {
                return true;
            }

            queue.push(process.process_id);
        }
    }

    false
}

/// Open the Codex desktop app if it is installed.
#[tauri::command]
pub async fn open_codex_app() -> Result<(), String> {
    tokio::task::spawn_blocking(open_codex_app_blocking)
        .await
        .map_err(|e| e.to_string())?
}

fn open_codex_app_blocking() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        if command_succeeds(Command::new("open").args(["-b", "com.openai.codex"])) {
            return Ok(());
        }

        if command_succeeds(Command::new("open").args(["-a", "Codex"])) {
            return Ok(());
        }

        return Err("Codex app is not installed or could not be opened".to_string());
    }

    #[cfg(windows)]
    {
        if open_windows_registered_app() {
            return Ok(());
        }

        if let Some(path) = find_windows_codex_app() {
            if spawn_windows_codex_exe(&path) {
                return Ok(());
            }
        }

        for shortcut in find_windows_codex_shortcuts() {
            if open_windows_shortcut(&shortcut) {
                return Ok(());
            }
        }

        return Err("Codex app is not installed or could not be opened".to_string());
    }

    #[allow(unreachable_code)]
    Err("Opening Codex app is only supported on macOS and Windows".to_string())
}

fn command_succeeds(command: &mut Command) -> bool {
    command
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn find_windows_codex_app() -> Option<std::path::PathBuf> {
    let mut candidates = Vec::new();

    for key in ["LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(base) = std::env::var_os(key) {
            let base = std::path::PathBuf::from(base);
            candidates.push(base.join("Programs").join("Codex").join("Codex.exe"));
            candidates.push(base.join("Programs").join("codex").join("Codex.exe"));
            candidates.push(base.join("Codex").join("Codex.exe"));
            candidates.push(base.join("OpenAI").join("Codex").join("Codex.exe"));
            candidates.push(base.join("OpenAI Codex").join("Codex.exe"));
            candidates.push(base.join("Codex Desktop").join("Codex.exe"));
        }
    }

    candidates.extend(find_windows_codex_apps_in_programs());

    candidates.into_iter().find(|path| {
        path.is_file()
            && looks_like_windows_desktop_app(path)
            && is_windows_legacy_codex_desktop_path(&path.to_string_lossy())
    })
}

#[cfg(windows)]
fn looks_like_windows_desktop_app(path: &std::path::Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };

    parent.join("resources").join("app.asar").is_file()
        || parent.join("resources").join("app").is_dir()
        || parent.join("resources").is_dir()
}

#[cfg(windows)]
fn spawn_windows_codex_exe(path: &std::path::Path) -> bool {
    let mut command = Command::new(path);
    command.creation_flags(CREATE_NO_WINDOW);
    if let Some(parent) = path.parent() {
        command.current_dir(parent);
    }
    command.spawn().is_ok()
}

#[cfg(windows)]
fn open_windows_registered_app() -> bool {
    let script = r#"
$app = Get-StartApps |
  Where-Object {
    $name = [string]$_.Name
    $appId = [string]$_.AppID
    $text = ($name + ' ' + $appId).ToLowerInvariant()
    $isSwitcher = $text.Contains('codex switcher') -or $text.Contains('codex-switcher') -or $text.Contains('lampese')
    $isCodex = $name -eq 'Codex' -or $name -eq 'OpenAI Codex' -or $appId -like 'OpenAI.Codex*' -or ($text.Contains('openai') -and $text.Contains('codex'))
    $isCodex -and -not $isSwitcher
  } |
  Sort-Object @{ Expression = {
    if ($_.AppID -like 'OpenAI.Codex*') { 0 }
    elseif ($_.Name -eq 'Codex') { 1 }
    elseif ($_.Name -eq 'OpenAI Codex') { 2 }
    else { 3 }
  } }, Name |
  Select-Object -First 1
if ($null -eq $app) { exit 1 }
Start-Process ("shell:AppsFolder\" + $app.AppID)
"#;

    let mut command = Command::new("powershell.exe");
    command.creation_flags(CREATE_NO_WINDOW);
    command.args(["-NoProfile", "-NonInteractive", "-Command", script]);
    command_succeeds(&mut command)
}

#[cfg(windows)]
fn find_windows_codex_shortcuts() -> Vec<std::path::PathBuf> {
    let mut candidates = Vec::new();

    for key in ["APPDATA", "ProgramData"] {
        if let Some(base) = std::env::var_os(key) {
            let programs = std::path::PathBuf::from(base)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs");
            candidates.push(programs.join("Codex.lnk"));
            candidates.push(programs.join("OpenAI").join("Codex.lnk"));
            collect_windows_codex_shortcuts(&programs, &mut candidates, 0);
        }
    }

    candidates
        .into_iter()
        .filter(|path| path.is_file())
        .collect()
}

#[cfg(windows)]
fn open_windows_shortcut(path: &std::path::Path) -> bool {
    let mut command = Command::new("cmd.exe");
    command.creation_flags(CREATE_NO_WINDOW);
    command.arg("/C").arg("start").arg("").arg(path);
    command_succeeds(&mut command)
}

#[cfg(windows)]
fn find_windows_codex_apps_in_programs() -> Vec<std::path::PathBuf> {
    let mut candidates = Vec::new();

    let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") else {
        return candidates;
    };

    let programs = std::path::PathBuf::from(local_app_data).join("Programs");
    collect_windows_codex_apps(&programs, &mut candidates, 0);
    candidates
}

#[cfg(windows)]
fn collect_windows_codex_apps(
    dir: &std::path::Path,
    candidates: &mut Vec<std::path::PathBuf>,
    depth: usize,
) {
    if depth > 2 {
        return;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_windows_codex_apps(&path, candidates, depth + 1);
            continue;
        }

        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        if file_name.eq_ignore_ascii_case("Codex.exe") {
            candidates.push(path);
        }
    }
}

#[cfg(windows)]
fn collect_windows_codex_shortcuts(
    dir: &std::path::Path,
    candidates: &mut Vec<std::path::PathBuf>,
    depth: usize,
) {
    if depth > 3 {
        return;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_windows_codex_shortcuts(&path, candidates, depth + 1);
            continue;
        }

        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        if is_windows_codex_shortcut_name(file_name) {
            candidates.push(path);
        }
    }
}

#[cfg(any(windows, test))]
fn is_windows_codex_shortcut_name(file_name: &str) -> bool {
    if !file_name
        .rsplit_once('.')
        .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("lnk"))
    {
        return false;
    }

    let shortcut_name = file_name
        .rsplit_once('.')
        .map(|(name, _)| name)
        .unwrap_or(file_name)
        .to_ascii_lowercase();

    if shortcut_name.contains("codex switcher")
        || shortcut_name.contains("codex-switcher")
        || shortcut_name.contains("switcher")
    {
        return false;
    }

    shortcut_name == "codex"
        || shortcut_name.starts_with("codex ")
        || shortcut_name.contains("openai codex")
        || (shortcut_name.contains("openai") && shortcut_name.contains("codex"))
}
