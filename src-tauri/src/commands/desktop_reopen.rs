//! Reopen only trusted Codex desktop apps observed immediately before closing.
//! Launch targets remain in the backend; the frontend receives a short-lived,
//! one-use token so it cannot choose an executable or application identity.

use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
enum DesktopTarget {
    #[cfg(any(target_os = "macos", test))]
    MacBundle(String),
    #[cfg(any(windows, test))]
    WindowsExecutable(String),
    #[cfg(any(windows, test))]
    WindowsAppId(String),
}

pub(super) struct CapturedDesktop {
    pid: u32,
    target: DesktopTarget,
}

struct ReopenTicket {
    token: String,
    created_at: Instant,
    targets: Vec<DesktopTarget>,
}

static PENDING_REOPEN: Mutex<Option<ReopenTicket>> = Mutex::new(None);

#[derive(Debug, Clone, serde::Serialize)]
pub struct CodexReopenInfo {
    pub supported: bool,
    pub desktop_count: usize,
}

#[tauri::command]
pub async fn get_codex_reopen_info() -> Result<CodexReopenInfo, String> {
    tokio::task::spawn_blocking(|| {
        let (pids, _) = super::find_codex_processes().map_err(|error| error.to_string())?;
        let desktops = capture_desktops(&pids)?;
        Ok(CodexReopenInfo {
            supported: cfg!(any(target_os = "macos", windows)),
            desktop_count: desktops.len(),
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

pub(super) fn capture_desktops(pids: &[u32]) -> Result<Vec<CapturedDesktop>, String> {
    let mut desktops = Vec::new();

    #[cfg(target_os = "macos")]
    {
        let names = super::read_unix_process_names();
        for &pid in pids {
            let output = super::Command::new("ps")
                .arg("-p")
                .arg(pid.to_string())
                .args(["-o", "command="])
                .output()
                .map_err(|error| error.to_string())?;
            if !output.status.success() {
                continue;
            }

            let command = String::from_utf8_lossy(&output.stdout);
            let process_name = names.get(&pid).map(String::as_str);
            let Some(bundle) = mac_bundle_path(command.trim(), process_name) else {
                continue;
            };
            if is_codex_bundle(&bundle) {
                desktops.push(CapturedDesktop {
                    pid,
                    target: DesktopTarget::MacBundle(bundle),
                });
            }
        }
    }

    #[cfg(windows)]
    {
        let processes =
            super::read_windows_process_snapshot().map_err(|error| error.to_string())?;
        for process in &processes {
            if !pids.contains(&process.process_id)
                || !super::is_windows_codex_root_process(process, &processes)
            {
                continue;
            }

            let path = process.executable_path.trim();
            let target = if super::is_windows_codex_package_root_path(path) {
                windows_app_id(path).map(DesktopTarget::WindowsAppId)
            } else if super::is_windows_legacy_codex_desktop_path(path) {
                Some(DesktopTarget::WindowsExecutable(path.to_string()))
            } else {
                None
            };
            if let Some(target) = target {
                desktops.push(CapturedDesktop {
                    pid: process.process_id,
                    target,
                });
            }
        }
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = (pids, &mut desktops);

    Ok(desktops)
}

#[cfg(any(target_os = "macos", test))]
fn mac_bundle_path(command: &str, process_name: Option<&str>) -> Option<String> {
    let executable_suffix = match process_name? {
        "ChatGPT" => "/ChatGPT.app/Contents/MacOS/ChatGPT",
        "Codex" => "/Codex.app/Contents/MacOS/Codex",
        _ => return None,
    };
    if !command.starts_with('/') {
        return None;
    }

    let executable_index = command.find(executable_suffix)?;
    if command[executable_index + executable_suffix.len()..]
        .chars()
        .next()
        .is_some_and(|character| !character.is_whitespace())
    {
        return None;
    }
    let bundle_suffix_end = executable_suffix.find("/Contents/")?;
    Some(command[..executable_index + bundle_suffix_end].to_string())
}

#[cfg(target_os = "macos")]
fn is_codex_bundle(bundle: &str) -> bool {
    plist::Value::from_file(std::path::Path::new(bundle).join("Contents/Info.plist"))
        .ok()
        .and_then(|value| {
            value
                .as_dictionary()?
                .get("CFBundleIdentifier")?
                .as_string()
                .map(str::to_owned)
        })
        .is_some_and(|identifier| identifier == "com.openai.codex")
}

#[cfg(windows)]
fn windows_app_id(executable: &str) -> Option<String> {
    use std::os::windows::process::CommandExt;

    let script = r#"
$ErrorActionPreference = 'Stop'
$exe = $env:CODEX_SWITCHER_REOPEN_EXE
foreach ($pkg in (Get-AppxPackage -Name 'OpenAI.Codex*')) {
  if (-not $pkg.InstallLocation) { continue }
  foreach ($app in (Get-AppxPackageManifest -Package $pkg.PackageFullName).Package.Applications.Application) {
    if (-not $app.Executable -or -not $app.Id) { continue }
    $candidate = Join-Path $pkg.InstallLocation $app.Executable
    if ([string]::Equals($candidate, $exe, [StringComparison]::OrdinalIgnoreCase)) {
      Write-Output ($pkg.PackageFamilyName + '!' + $app.Id)
      exit 0
    }
  }
}
exit 1
"#;
    let output = super::Command::new("powershell.exe")
        .creation_flags(super::CREATE_NO_WINDOW)
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("CODEX_SWITCHER_REOPEN_EXE", executable)
        .output()
        .ok()?;
    let app_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && valid_windows_app_id(&app_id)).then_some(app_id)
}

#[cfg(any(windows, test))]
fn valid_windows_app_id(app_id: &str) -> bool {
    app_id.to_ascii_lowercase().starts_with("openai.codex")
        && app_id.contains("_2p2nqsd0c76g0!")
        && app_id.split('!').count() == 2
        && !app_id.ends_with('!')
        && app_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-!".contains(character))
}

fn closed_targets(desktops: Vec<CapturedDesktop>, closed_pids: &[u32]) -> Vec<DesktopTarget> {
    let mut targets = Vec::new();
    for desktop in desktops {
        if closed_pids.contains(&desktop.pid) && !targets.contains(&desktop.target) {
            targets.push(desktop.target);
        }
    }
    targets
}

pub(super) fn remember_closed_desktops(
    desktops: Vec<CapturedDesktop>,
    closed_pids: &[u32],
) -> Option<String> {
    let targets = closed_targets(desktops, closed_pids);
    let mut pending = PENDING_REOPEN.lock().ok()?;
    *pending = None;
    if targets.is_empty() {
        return None;
    }

    let token = uuid::Uuid::new_v4().to_string();
    *pending = Some(ReopenTicket {
        token: token.clone(),
        created_at: Instant::now(),
        targets,
    });
    Some(token)
}

fn take_targets(
    pending: &mut Option<ReopenTicket>,
    token: &str,
) -> Result<Vec<DesktopTarget>, String> {
    let valid = pending.as_ref().is_some_and(|ticket| {
        ticket.token == token && ticket.created_at.elapsed() < Duration::from_secs(120)
    });
    if !valid {
        return Err("The desktop reopen request expired or is no longer available".into());
    }
    Ok(pending.take().expect("validated reopen ticket").targets)
}

#[tauri::command]
pub async fn reopen_closed_codex_desktop(token: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let targets = take_targets(
            &mut *PENDING_REOPEN.lock().map_err(|error| error.to_string())?,
            &token,
        )?;
        super::ensure_codex_not_running()?;

        for target in &targets {
            if !launch_desktop(target) {
                return Err(
                    "Could not launch the captured Codex desktop. Open it manually.".into(),
                );
            }
        }

        let expected = targets.clone();
        if !wait_for_desktops(&expected, Duration::from_secs(20), || {
            let (pids, _) = super::find_codex_processes().map_err(|error| error.to_string())?;
            Ok(capture_desktops(&pids)?
                .into_iter()
                .map(|desktop| desktop.target)
                .collect())
        }) {
            return Err("Codex desktop did not appear within 20 seconds. Open it manually.".into());
        }
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
}

fn wait_for_desktops(
    expected: &[DesktopTarget],
    timeout: Duration,
    mut inspect: impl FnMut() -> Result<Vec<DesktopTarget>, String>,
) -> bool {
    let started = Instant::now();
    loop {
        if let Ok(running) = inspect() {
            if expected.iter().all(|target| running.contains(target)) {
                return true;
            }
        }
        if started.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn launch_desktop(target: &DesktopTarget) -> bool {
    match target {
        #[cfg(target_os = "macos")]
        DesktopTarget::MacBundle(bundle) => {
            is_codex_bundle(bundle)
                && super::command_succeeds(super::Command::new("open").arg("-a").arg(bundle))
        }
        #[cfg(windows)]
        DesktopTarget::WindowsExecutable(path) => {
            let path = std::path::Path::new(path);
            super::is_windows_legacy_codex_desktop_path(&path.to_string_lossy())
                && super::looks_like_windows_desktop_app(path)
                && super::spawn_windows_codex_exe(path)
        }
        #[cfg(windows)]
        DesktopTarget::WindowsAppId(app_id) => {
            use std::os::windows::process::CommandExt;
            valid_windows_app_id(app_id)
                && super::command_succeeds(
                    super::Command::new("powershell.exe")
                        .creation_flags(super::CREATE_NO_WINDOW)
                        .args([
                            "-NoProfile",
                            "-NonInteractive",
                            "-Command",
                            "$ErrorActionPreference = 'Stop'; Start-Process ('shell:AppsFolder\\' + $env:CODEX_SWITCHER_REOPEN_APP_ID)",
                        ])
                        .env("CODEX_SWITCHER_REOPEN_APP_ID", app_id),
                )
        }
        #[allow(unreachable_patterns)]
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_exact_macos_bundle_and_rejects_helpers() {
        assert_eq!(
            mac_bundle_path(
                "/Users/test/My Apps/ChatGPT.app/Contents/MacOS/ChatGPT --flag",
                Some("ChatGPT"),
            ),
            Some("/Users/test/My Apps/ChatGPT.app".into()),
        );
        assert_eq!(
            mac_bundle_path(
                "/Applications/Codex.app/Contents/MacOS/Codex",
                Some("Codex"),
            ),
            Some("/Applications/Codex.app".into()),
        );
        assert_eq!(
            mac_bundle_path(
                "/Applications/ChatGPT.app/Contents/MacOS/ChatGPTHelper",
                Some("ChatGPT"),
            ),
            None,
        );
        assert_eq!(mac_bundle_path("codex", Some("codex")), None);
    }

    #[test]
    fn windows_app_id_rejects_other_apps_and_malformed_identifiers() {
        assert!(valid_windows_app_id("OpenAI.Codex_2p2nqsd0c76g0!App"));
        for app_id in [
            "OpenAI.ChatGPT_2p2nqsd0c76g0!App",
            "OpenAI.Codex_other!App",
            "OpenAI.Codex_2p2nqsd0c76g0!",
            "OpenAI.Codex_2p2nqsd0c76g0!App\n",
            "OpenAI.Codex_2p2nqsd0c76g0!App!Other",
        ] {
            assert!(!valid_windows_app_id(app_id));
        }
    }

    #[test]
    fn reopens_only_closed_roots_and_deduplicates_installations() {
        let target = DesktopTarget::MacBundle("/Applications/ChatGPT.app".into());
        let desktops = vec![
            CapturedDesktop {
                pid: 1,
                target: target.clone(),
            },
            CapturedDesktop {
                pid: 2,
                target: target.clone(),
            },
            CapturedDesktop {
                pid: 3,
                target: DesktopTarget::MacBundle("/Other/Codex.app".into()),
            },
        ];
        assert_eq!(closed_targets(desktops, &[1, 2]), vec![target]);
    }

    #[test]
    fn reopen_tickets_are_one_use_and_expire() {
        let mut ticket = Some(ReopenTicket {
            token: "token".into(),
            created_at: Instant::now(),
            targets: vec![DesktopTarget::WindowsAppId(
                "OpenAI.Codex_2p2nqsd0c76g0!App".into(),
            )],
        });
        assert!(take_targets(&mut ticket, "wrong").is_err());
        assert_eq!(take_targets(&mut ticket, "token").unwrap().len(), 1);
        assert!(take_targets(&mut ticket, "token").is_err());

        let mut expired = Some(ReopenTicket {
            token: "expired".into(),
            created_at: Instant::now() - Duration::from_secs(121),
            targets: vec![DesktopTarget::WindowsExecutable("C:\\Codex.exe".into())],
        });
        assert!(take_targets(&mut expired, "expired").is_err());
    }

    #[test]
    fn launch_confirmation_requires_the_exact_desktop() {
        let expected = vec![DesktopTarget::MacBundle("/Applications/ChatGPT.app".into())];
        assert!(wait_for_desktops(&expected, Duration::ZERO, || Ok(
            expected.clone()
        )));
        assert!(!wait_for_desktops(&expected, Duration::ZERO, || Ok(vec![])));
        assert!(!wait_for_desktops(&expected, Duration::ZERO, || Err(
            "query failed".into()
        )));
    }
}
