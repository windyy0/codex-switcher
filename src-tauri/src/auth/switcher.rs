//! Account switching logic - writes credentials to ~/.codex/auth.json

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::Utc;
use sha2::{Digest, Sha256};
use toml_edit::{DocumentMut, Item, Table};

use super::{get_config_dir, load_accounts, save_accounts, write_file_atomic};
use crate::types::{
    parse_chatgpt_id_token_claims, AccountsStore, AuthData, AuthDotJson, CodexConfigManagedState,
    CodexConfigTransitionJournal, StoredAccount, TokenData,
};

/// Get the official Codex home directory
pub fn get_codex_home() -> Result<PathBuf> {
    // Check for CODEX_HOME environment variable first
    if let Ok(codex_home) = std::env::var("CODEX_HOME") {
        return Ok(PathBuf::from(codex_home));
    }

    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".codex"))
}

pub fn get_codex_home_identity() -> Result<String> {
    let codex_home = get_codex_home()?;
    fs::create_dir_all(&codex_home)?;
    Ok(fs::canonicalize(&codex_home)?
        .to_string_lossy()
        .into_owned())
}

/// Get the path to the official auth.json file
pub fn get_codex_auth_file() -> Result<PathBuf> {
    Ok(get_codex_home()?.join("auth.json"))
}

/// Switch to a specific account by writing its credentials to ~/.codex/auth.json
pub fn switch_to_account(account: &StoredAccount) -> Result<()> {
    switch_to_account_transaction(account, None)
}

/// Synchronize only the derived auth.json after a durable token rotation.
/// Token refresh must never touch config.toml.
pub fn sync_account_auth_file(account: &StoredAccount) -> Result<()> {
    let codex_home = get_codex_home()?;
    sync_account_auth_file_at_home(account, &codex_home)
}

pub fn sync_account_auth_file_at_home(account: &StoredAccount, codex_home: &Path) -> Result<()> {
    fs::create_dir_all(codex_home)?;
    write_account_auth(&codex_home.join("auth.json"), account)
}

fn write_account_auth(auth_path: &Path, account: &StoredAccount) -> Result<()> {
    let auth_json = create_auth_json(account)?;
    let content = serde_json::to_vec_pretty(&auth_json).context("Failed to serialize auth.json")?;
    write_file_atomic(auth_path, &content)
        .with_context(|| format!("Failed to write auth.json: {}", auth_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(auth_path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub fn switch_to_account_removing(account: &StoredAccount, remove_account_id: &str) -> Result<()> {
    switch_to_account_transaction(account, Some(remove_account_id))
}

fn switch_to_account_transaction(
    account: &StoredAccount,
    remove_account_id: Option<&str>,
) -> Result<()> {
    let codex_home = get_codex_home()?;

    // Ensure the codex home directory exists
    fs::create_dir_all(&codex_home)
        .with_context(|| format!("Failed to create codex home: {}", codex_home.display()))?;

    let auth_path = codex_home.join("auth.json");
    let config_path = codex_home.join("config.toml");
    let backup_path = get_config_dir()?.join("config.toml.backup");
    transition_with_rollback(&[&auth_path, &config_path, &backup_path], || {
        // Validate, back up, merge, and write config before changing credentials.
        sync_codex_config(
            &codex_home,
            account.codex_config.as_deref(),
            Some(account),
            remove_account_id,
        )?;
        write_account_auth(&auth_path, account)?;
        Ok(())
    })?;

    Ok(())
}

/// Remove active credentials and restore the normal config when the last
/// managed account is deleted.
pub fn clear_codex_account_files() -> Result<()> {
    clear_codex_account_files_transaction(None)
}

pub fn clear_codex_account_files_removing(remove_account_id: &str) -> Result<()> {
    clear_codex_account_files_transaction(Some(remove_account_id))
}

fn clear_codex_account_files_transaction(remove_account_id: Option<&str>) -> Result<()> {
    let codex_home = get_codex_home()?;
    fs::create_dir_all(&codex_home)
        .with_context(|| format!("Failed to create codex home: {}", codex_home.display()))?;

    let auth_path = codex_home.join("auth.json");
    let config_path = codex_home.join("config.toml");
    let backup_path = get_config_dir()?.join("config.toml.backup");

    transition_with_rollback(&[&auth_path, &config_path, &backup_path], || {
        sync_codex_config(&codex_home, None, None, remove_account_id)?;
        if auth_path.exists() {
            fs::remove_file(&auth_path)
                .with_context(|| format!("Failed to remove auth.json: {}", auth_path.display()))?;
        }
        Ok(())
    })?;

    Ok(())
}

/// Snapshot all files and store state touched by a higher-level account
/// transition. Commands keep this until active-account metadata is committed,
/// allowing a complete rollback even when there was no previous managed account.
pub struct CodexStateSnapshot {
    files: Vec<FileSnapshot>,
    store: crate::types::AccountsStore,
}

pub fn snapshot_codex_state() -> Result<CodexStateSnapshot> {
    let codex_home = get_codex_home()?;
    let paths = [
        codex_home.join("auth.json"),
        codex_home.join("config.toml"),
        get_config_dir()?.join("config.toml.backup"),
    ];
    let files = paths
        .iter()
        .map(|path| FileSnapshot::capture(path))
        .collect::<Result<Vec<_>>>()?;
    Ok(CodexStateSnapshot {
        files,
        store: load_accounts()?,
    })
}

pub fn restore_codex_state(snapshot: &CodexStateSnapshot) -> Result<()> {
    let mut errors = Vec::new();
    for file in &snapshot.files {
        if let Err(error) = file.restore() {
            errors.push(error.to_string());
        }
    }
    if let Err(error) = save_accounts(&snapshot.store) {
        errors.push(error.to_string());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("Failed to restore Codex state: {}", errors.join("; "))
    }
}

/// Apply an API account's config.toml while preserving the user's normal
/// configuration, then restore it when switching back to a regular account.
fn sync_codex_config(
    codex_home: &Path,
    account_config: Option<&str>,
    target_account: Option<&StoredAccount>,
    remove_account_id: Option<&str>,
) -> Result<()> {
    let config_path = codex_home.join("config.toml");
    let backup_path = get_config_dir()?.join("config.toml.backup");
    let mut store = load_accounts()?;
    let home_key = fs::canonicalize(codex_home)
        .with_context(|| format!("Failed to resolve CODEX_HOME: {}", codex_home.display()))?
        .to_string_lossy()
        .into_owned();
    let current_config =
        if config_path.is_file() {
            Some(fs::read_to_string(&config_path).with_context(|| {
                format!("Failed to read config.toml: {}", config_path.display())
            })?)
        } else {
            None
        };

    recover_codex_config_transition(
        &mut store,
        &home_key,
        current_config.as_deref(),
        &backup_path,
        &config_path,
    )?;

    // Move an inline backup created by an earlier development build into the
    // dedicated file before validating its legacy home identity. This ensures
    // the recovery instructions always point at a backup that actually exists.
    if store.codex_config_backup_captured {
        if let Some(legacy_backup) = store.codex_config_backup.clone() {
            if store.codex_config_backup_existed && !backup_path.is_file() {
                write_config_backup(&backup_path, &legacy_backup)?;
            }
            store.codex_config_backup = None;
            save_accounts(&store)?;
        }
    }

    if store.codex_config_backup_captured {
        match store.codex_config_backup_home.as_deref() {
            Some(backup_home) if backup_home != home_key => {
                anyhow::bail!(
                    "The active config backup belongs to a different CODEX_HOME ({backup_home}); restore that environment before switching {}",
                    codex_home.display()
                );
            }
            None => {
                if store.codex_config_backup_existed {
                    anyhow::bail!(
                        "The legacy config backup has no CODEX_HOME identity; refusing to apply it automatically. Restore it from {} while using the original CODEX_HOME, then switch again",
                        backup_path.display()
                    );
                }
                anyhow::bail!(
                    "The legacy config backup has no CODEX_HOME identity and records that config.toml originally did not exist; use the original CODEX_HOME to remove the managed config.toml before switching again"
                );
            }
            _ => {}
        }
    }

    let was_captured = store.codex_config_backup_captured;

    if let Some(config) = account_config {
        let backup_existed;
        if !was_captured {
            let existed = config_path.is_file();
            backup_existed = existed;
            if existed {
                let backup = current_config.as_deref().unwrap_or_default();
                write_config_backup(&backup_path, backup)?;
            } else if backup_path.exists() {
                fs::remove_file(&backup_path).with_context(|| {
                    format!(
                        "Failed to clear stale config backup: {}",
                        backup_path.display()
                    )
                })?;
            }
        } else {
            backup_existed = store.codex_config_backup_existed;
        }

        let baseline_config = if backup_existed {
            if was_captured {
                Some(read_config_backup(&backup_path, &store)?)
            } else {
                current_config.clone()
            }
        } else {
            None
        };
        let previous_overlay = store.codex_config_active_overlay.clone().or_else(|| {
            store
                .active_account_id
                .as_deref()
                .and_then(|active_id| {
                    store
                        .accounts
                        .iter()
                        .find(|account| account.id == active_id)
                })
                .and_then(|account| account.codex_config.clone())
        });
        let normal_config = if was_captured {
            match previous_overlay.as_deref() {
                Some(overlay) => strip_codex_config_overlay(
                    baseline_config.as_deref(),
                    current_config.as_deref(),
                    overlay,
                )?,
                None => anyhow::bail!(
                    "Cannot identify the API config currently applied to {}",
                    config_path.display()
                ),
            }
        } else {
            current_config.clone().unwrap_or_default()
        };
        let merged_config = merge_codex_config(Some(&normal_config), config)?;
        let after_state = CodexConfigManagedState {
            backup_captured: true,
            backup_existed,
            backup_home: Some(home_key.clone()),
            active_overlay: Some(config.to_string()),
        };
        commit_codex_config_transition(
            &mut store,
            &home_key,
            current_config.as_deref(),
            Some(&merged_config),
            after_state,
            &config_path,
            &backup_path,
            target_account,
            remove_account_id,
        )?;
        return Ok(());
    }

    if !store.codex_config_backup_captured {
        let state = managed_config_state(&store);
        commit_codex_config_transition(
            &mut store,
            &home_key,
            current_config.as_deref(),
            current_config.as_deref(),
            state,
            &config_path,
            &backup_path,
            target_account,
            remove_account_id,
        )?;
        return Ok(());
    }

    let baseline_config = if store.codex_config_backup_existed {
        Some(read_config_backup(&backup_path, &store)?)
    } else {
        None
    };
    let previous_overlay = store.codex_config_active_overlay.clone().or_else(|| {
        store
            .active_account_id
            .as_deref()
            .and_then(|active_id| {
                store
                    .accounts
                    .iter()
                    .find(|account| account.id == active_id)
            })
            .and_then(|account| account.codex_config.clone())
    });
    let overlay = previous_overlay.context("Cannot identify the active API config overlay")?;
    let normal_config = strip_codex_config_overlay(
        baseline_config.as_deref(),
        current_config.as_deref(),
        &overlay,
    )?;

    let restored_config = if !store.codex_config_backup_existed && normal_config.trim().is_empty() {
        None
    } else {
        Some(normal_config)
    };
    commit_codex_config_transition(
        &mut store,
        &home_key,
        current_config.as_deref(),
        restored_config.as_deref(),
        CodexConfigManagedState {
            backup_captured: false,
            backup_existed: false,
            backup_home: None,
            active_overlay: None,
        },
        &config_path,
        &backup_path,
        target_account,
        remove_account_id,
    )?;
    Ok(())
}

fn managed_config_state(store: &crate::types::AccountsStore) -> CodexConfigManagedState {
    CodexConfigManagedState {
        backup_captured: store.codex_config_backup_captured,
        backup_existed: store.codex_config_backup_existed,
        backup_home: store.codex_config_backup_home.clone(),
        active_overlay: store.codex_config_active_overlay.clone(),
    }
}

fn apply_managed_config_state(
    store: &mut crate::types::AccountsStore,
    state: &CodexConfigManagedState,
) {
    store.codex_config_backup_captured = state.backup_captured;
    store.codex_config_backup_existed = state.backup_existed;
    store.codex_config_backup_home = state.backup_home.clone();
    store.codex_config_active_overlay = state.active_overlay.clone();
    store.codex_config_backup = None;
}

fn config_hash(contents: Option<&str>) -> Option<String> {
    contents.map(|contents| URL_SAFE_NO_PAD.encode(Sha256::digest(contents.as_bytes())))
}

fn config_write_required(before: Option<&str>, after: Option<&str>) -> bool {
    before != after
}

fn recovered_managed_state<'a>(
    journal: &'a CodexConfigTransitionJournal,
    current_hash: &Option<String>,
) -> Result<&'a CodexConfigManagedState> {
    // This transition intentionally keeps config.toml untouched. External
    // project/trust updates are therefore compatible with recovery.
    if journal.before_config_hash == journal.after_config_hash {
        return Ok(&journal.after);
    }
    if current_hash == &journal.after_config_hash {
        Ok(&journal.after)
    } else if current_hash == &journal.before_config_hash {
        Ok(&journal.before)
    } else {
        anyhow::bail!(
            "config.toml changed during an interrupted account switch; refusing automatic recovery"
        )
    }
}

fn apply_journal_store_delta(
    store: &mut crate::types::AccountsStore,
    journal: &CodexConfigTransitionJournal,
) -> Result<Option<StoredAccount>> {
    let target_account = journal.target_account.clone().or_else(|| {
        journal.target_account_id.as_deref().and_then(|account_id| {
            store
                .accounts
                .iter()
                .find(|account| account.id == account_id)
                .cloned()
        })
    });
    if journal.target_account_id.is_some() && target_account.is_none() {
        anyhow::bail!("The target account for an interrupted switch is missing");
    }

    if let Some(account) = target_account.as_ref() {
        if let Some(existing) = store
            .accounts
            .iter_mut()
            .find(|existing| existing.id == account.id)
        {
            *existing = account.clone();
        } else {
            store.accounts.push(account.clone());
        }
        store.active_account_id = Some(account.id.clone());
        store.active_account_home = Some(journal.home.clone());
    } else {
        store.active_account_id = None;
        store.active_account_home = None;
    }
    if let Some(remove_account_id) = journal.remove_account_id.as_deref() {
        store
            .accounts
            .retain(|account| account.id != remove_account_id);
    }
    Ok(target_account)
}

fn recover_codex_config_transition(
    store: &mut crate::types::AccountsStore,
    home_key: &str,
    current_config: Option<&str>,
    backup_path: &Path,
    config_path: &Path,
) -> Result<()> {
    let Some(journal) = store.codex_config_transition.clone() else {
        return Ok(());
    };
    if journal.home != home_key {
        anyhow::bail!(
            "The pending config transition belongs to a different CODEX_HOME ({})",
            journal.home
        );
    }

    let current_hash = config_hash(current_config);
    let recovered_state = recovered_managed_state(&journal, &current_hash)?;
    let complete_after = journal.before_config_hash == journal.after_config_hash
        || current_hash == journal.after_config_hash;

    // A matching after hash means config.toml committed before the process
    // stopped. Complete the remaining auth.json + active-account portion of
    // the same transition before clearing the journal.
    if complete_after {
        let auth_path = config_path.with_file_name("auth.json");
        let target_account = apply_journal_store_delta(store, &journal)?;
        match target_account.as_ref() {
            Some(account) => {
                let auth = create_auth_json(account)?;
                let contents = serde_json::to_vec_pretty(&auth)?;
                write_file_atomic(&auth_path, &contents)?;
            }
            None => {
                if auth_path.exists() {
                    fs::remove_file(&auth_path)?;
                }
            }
        }
    }

    apply_managed_config_state(store, recovered_state);
    store.codex_config_transition = None;
    save_accounts(store)?;
    if !recovered_state.backup_captured && backup_path.exists() {
        fs::remove_file(backup_path).with_context(|| {
            format!(
                "Failed to remove stale config backup: {}",
                backup_path.display()
            )
        })?;
    }
    Ok(())
}

fn commit_codex_config_transition(
    store: &mut crate::types::AccountsStore,
    home_key: &str,
    before_config: Option<&str>,
    after_config: Option<&str>,
    after_state: CodexConfigManagedState,
    config_path: &Path,
    backup_path: &Path,
    target_account: Option<&StoredAccount>,
    remove_account_id: Option<&str>,
) -> Result<()> {
    let before_state = managed_config_state(store);
    store.codex_config_transition = Some(CodexConfigTransitionJournal {
        home: home_key.to_string(),
        target_account_id: target_account.map(|account| account.id.clone()),
        target_account: target_account.cloned(),
        remove_account_id: remove_account_id.map(str::to_string),
        before_config_hash: config_hash(before_config),
        after_config_hash: config_hash(after_config),
        before: before_state,
        after: after_state.clone(),
    });
    save_accounts(store)?;

    if config_write_required(before_config, after_config) {
        match after_config {
            Some(contents) => {
                write_file_atomic(config_path, contents.as_bytes()).with_context(|| {
                    format!("Failed to write config.toml: {}", config_path.display())
                })?
            }
            None if config_path.exists() => fs::remove_file(config_path).with_context(|| {
                format!("Failed to remove config.toml: {}", config_path.display())
            })?,
            None => {}
        }
    }

    apply_managed_config_state(store, &after_state);
    // Keep the journal until auth.json and active_account_id are committed by
    // the higher-level account operation.
    save_accounts(store)?;
    if !after_state.backup_captured && backup_path.exists() {
        fs::remove_file(backup_path).with_context(|| {
            format!("Failed to remove config backup: {}", backup_path.display())
        })?;
    }
    Ok(())
}

/// Finish a successful cross-file account transition. Call this only after
/// auth.json and active_account_id have both reached their intended values.
pub fn finalize_codex_transition() -> Result<()> {
    let mut store = load_accounts()?;
    if store.codex_config_transition.is_some()
        || store.pending_auth_sync_account_id.is_some()
        || store.pending_auth_sync_home.is_some()
    {
        if let Some(journal) = store.codex_config_transition.as_ref() {
            store.active_account_home = journal
                .target_account_id
                .as_ref()
                .map(|_| journal.home.clone());
        }
        store.codex_config_transition = None;
        store.pending_auth_sync_account_id = None;
        store.pending_auth_sync_home = None;
        save_accounts(&store)?;
    }
    Ok(())
}

/// Finish only the derived auth.json synchronization for one account. Token
/// rotation must never clear an unrelated config/account transition journal.
pub fn finalize_account_auth_sync(account_id: &str, home: &str) -> Result<()> {
    let mut store = load_accounts()?;
    if clear_pending_auth_sync(&mut store, account_id, home) {
        save_accounts(&store)?;
    }
    Ok(())
}

fn clear_pending_auth_sync(store: &mut AccountsStore, account_id: &str, home: &str) -> bool {
    if store.pending_auth_sync_account_id.as_deref() != Some(account_id)
        || store
            .pending_auth_sync_home
            .as_deref()
            .is_some_and(|pending_home| pending_home != home)
    {
        return false;
    }
    store.pending_auth_sync_account_id = None;
    store.pending_auth_sync_home = None;
    true
}

/// Reconcile durable account-transition state before any UI, tray, or external
/// Codex process can observe a partially committed switch.
pub fn recover_pending_account_transition() -> Result<()> {
    let codex_home = get_codex_home()?;
    fs::create_dir_all(&codex_home)?;
    let config_path = codex_home.join("config.toml");
    let backup_path = get_config_dir()?.join("config.toml.backup");
    let home_key = fs::canonicalize(&codex_home)?
        .to_string_lossy()
        .into_owned();
    let current_config = if config_path.is_file() {
        Some(fs::read_to_string(&config_path)?)
    } else {
        None
    };
    let mut store = load_accounts()?;
    recover_codex_config_transition(
        &mut store,
        &home_key,
        current_config.as_deref(),
        &backup_path,
        &config_path,
    )?;

    let mut store = load_accounts()?;
    if let Some(account_id) = store.pending_auth_sync_account_id.clone() {
        let sync_home = store
            .pending_auth_sync_home
            .as_deref()
            .or(store.active_account_home.as_deref())
            .map(PathBuf::from)
            .unwrap_or_else(|| codex_home.clone());
        let sync_home_key = sync_home.to_string_lossy();
        let home_is_still_active = store
            .active_account_home
            .as_deref()
            .is_none_or(|active_home| active_home == sync_home_key.as_ref());
        if home_is_still_active && store.active_account_id.as_deref() == Some(account_id.as_str()) {
            if let Some(account) = store
                .accounts
                .iter()
                .find(|account| account.id == account_id)
            {
                fs::create_dir_all(&sync_home)?;
                let auth = create_auth_json(account)?;
                write_file_atomic(
                    &sync_home.join("auth.json"),
                    &serde_json::to_vec_pretty(&auth)?,
                )?;
            }
        }
        store.pending_auth_sync_account_id = None;
        store.pending_auth_sync_home = None;
        save_accounts(&store)?;
    } else if store.pending_auth_sync_home.take().is_some() {
        save_accounts(&store)?;
    }
    Ok(())
}

#[derive(Debug)]
struct FileSnapshot {
    path: PathBuf,
    contents: Option<Vec<u8>>,
    permissions: Option<fs::Permissions>,
}

impl FileSnapshot {
    fn capture(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self {
                path: path.to_path_buf(),
                contents: None,
                permissions: None,
            });
        }

        Ok(Self {
            path: path.to_path_buf(),
            contents: Some(
                fs::read(path)
                    .with_context(|| format!("Failed to snapshot file: {}", path.display()))?,
            ),
            permissions: Some(fs::metadata(path)?.permissions()),
        })
    }

    fn restore(&self) -> Result<()> {
        match &self.contents {
            Some(contents) => {
                if let Some(parent) = self.path.parent() {
                    fs::create_dir_all(parent)?;
                }
                write_file_atomic(&self.path, contents).with_context(|| {
                    format!("Failed to restore file snapshot: {}", self.path.display())
                })?;
                if let Some(permissions) = &self.permissions {
                    fs::set_permissions(&self.path, permissions.clone())?;
                }
            }
            None if self.path.exists() => {
                fs::remove_file(&self.path).with_context(|| {
                    format!("Failed to remove rolled-back file: {}", self.path.display())
                })?;
            }
            None => {}
        }
        Ok(())
    }
}

fn transition_with_rollback<F>(paths: &[&Path], operation: F) -> Result<()>
where
    F: FnOnce() -> Result<()>,
{
    let store_snapshot = load_accounts()?;
    let snapshots = paths
        .iter()
        .map(|path| FileSnapshot::capture(path))
        .collect::<Result<Vec<_>>>()?;

    if let Err(error) = operation() {
        let mut rollback_errors = Vec::new();
        for snapshot in &snapshots {
            if let Err(rollback_error) = snapshot.restore() {
                rollback_errors.push(rollback_error.to_string());
            }
        }
        if let Err(rollback_error) = save_accounts(&store_snapshot) {
            rollback_errors.push(rollback_error.to_string());
        }

        if rollback_errors.is_empty() {
            return Err(error);
        }
        anyhow::bail!(
            "{error:#}; rollback also failed: {}",
            rollback_errors.join("; ")
        );
    }
    Ok(())
}

fn write_config_backup(path: &std::path::Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create config backup directory: {}",
                parent.display()
            )
        })?;
    }
    write_file_atomic(path, contents.as_bytes())
        .with_context(|| format!("Failed to write config backup: {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn read_config_backup(
    path: &std::path::Path,
    store: &crate::types::AccountsStore,
) -> Result<String> {
    if path.is_file() {
        return fs::read_to_string(path)
            .with_context(|| format!("Failed to read config backup: {}", path.display()));
    }

    // Migration fallback for development builds that embedded the complete
    // backup in accounts.json before a dedicated backup file was introduced.
    if let Some(legacy_backup) = store.codex_config_backup.as_deref() {
        write_config_backup(path, legacy_backup)?;
        return Ok(legacy_backup.to_string());
    }

    anyhow::bail!(
        "The original config.toml backup is missing: {}",
        path.display()
    )
}

/// Validate a per-account config fragment before storing it.
pub fn validate_codex_config(config: &str) -> Result<()> {
    config
        .parse::<DocumentMut>()
        .context("Invalid config.toml syntax")?;
    Ok(())
}

/// Overlay an API config fragment on top of the user's normal Codex config.
/// Tables are merged recursively so sections such as `[features]` retain keys
/// that the API fragment does not mention. Values and arrays are replaced.
fn merge_codex_config(base: Option<&str>, overlay: &str) -> Result<String> {
    let mut document = match base {
        Some(contents) => contents
            .parse::<DocumentMut>()
            .context("The existing config.toml is invalid")?,
        None => DocumentMut::new(),
    };
    let overlay_document = overlay
        .parse::<DocumentMut>()
        .context("The API config.toml fragment is invalid")?;

    merge_toml_table(document.as_table_mut(), overlay_document.as_table());
    Ok(document.to_string())
}

/// Remove only keys owned by the active API overlay. Unrelated changes made
/// while API mode was active (for example new `[projects]` entries) survive.
fn strip_codex_config_overlay(
    baseline: Option<&str>,
    current: Option<&str>,
    overlay: &str,
) -> Result<String> {
    let baseline_text = baseline.unwrap_or_default();
    let Some(current_text) = current else {
        return Ok(baseline_text.to_string());
    };

    // Preserve the original file byte-for-byte when nothing changed after the
    // app wrote the merged API configuration.
    let expected = merge_codex_config(baseline, overlay)?;
    if current_text == expected {
        return Ok(baseline_text.to_string());
    }

    let baseline_document = baseline_text
        .parse::<DocumentMut>()
        .context("The backed-up config.toml is invalid")?;
    let mut current_document = current_text
        .parse::<DocumentMut>()
        .context("The current config.toml is invalid")?;
    let overlay_document = overlay
        .parse::<DocumentMut>()
        .context("The active API config.toml fragment is invalid")?;

    restore_overlay_table(
        current_document.as_table_mut(),
        baseline_document.as_table(),
        overlay_document.as_table(),
    );
    Ok(current_document.to_string())
}

fn restore_overlay_table(current: &mut Table, baseline: &Table, overlay: &Table) {
    for (key, overlay_item) in overlay.iter() {
        let baseline_item = baseline.get(key);
        if let Item::Table(overlay_table) = overlay_item {
            let mut replacement: Option<Option<Item>> = None;
            let remove_empty = match current.get_mut(key) {
                Some(Item::Table(current_table)) => match baseline_item {
                    Some(Item::Table(baseline_table)) => {
                        restore_overlay_table(current_table, baseline_table, overlay_table);
                        false
                    }
                    None => {
                        let empty_baseline = Table::new();
                        restore_overlay_table(current_table, &empty_baseline, overlay_table);
                        current_table.is_empty()
                    }
                    Some(item) => {
                        replacement = Some(Some(item.clone()));
                        false
                    }
                },
                Some(_) | None => {
                    replacement = Some(baseline_item.cloned());
                    false
                }
            };
            if let Some(replacement) = replacement {
                if let Some(item) = replacement {
                    current.insert(key, item);
                } else {
                    current.remove(key);
                }
            } else if remove_empty {
                current.remove(key);
            }
        } else if let Some(item) = baseline_item {
            current.insert(key, item.clone());
        } else {
            current.remove(key);
        }
    }
}

fn merge_toml_table(base: &mut Table, overlay: &Table) {
    for (key, overlay_item) in overlay.iter() {
        match (base.get_mut(key), overlay_item) {
            (Some(Item::Table(base_table)), Item::Table(overlay_table)) => {
                merge_toml_table(base_table, overlay_table);
            }
            _ => {
                base.insert(key, overlay_item.clone());
            }
        }
    }
}

/// Create an AuthDotJson structure from a StoredAccount
fn create_auth_json(account: &StoredAccount) -> Result<AuthDotJson> {
    match &account.auth_data {
        AuthData::ApiKey { key } => Ok(AuthDotJson {
            openai_api_key: Some(key.clone()),
            tokens: None,
            last_refresh: None,
        }),
        AuthData::ChatGPT {
            id_token,
            access_token,
            refresh_token,
            account_id,
        } => Ok(AuthDotJson {
            openai_api_key: None,
            tokens: Some(TokenData {
                id_token: id_token.clone(),
                access_token: access_token.clone(),
                refresh_token: refresh_token.clone(),
                account_id: account_id.clone(),
            }),
            last_refresh: Some(Utc::now()),
        }),
    }
}

/// Import an account from an existing auth.json file
pub fn import_from_auth_json(path: &str, account_name: String) -> Result<StoredAccount> {
    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read auth.json: {path}"))?;

    import_from_auth_json_contents(&content, account_name)
        .with_context(|| format!("Failed to parse auth.json: {path}"))
}

/// Import an account from auth.json file contents.
pub fn import_from_auth_json_contents(
    content: &str,
    account_name: String,
) -> Result<StoredAccount> {
    let auth: AuthDotJson =
        serde_json::from_str(&content).context("Failed to parse auth.json contents")?;

    // Determine auth mode and create account
    if let Some(api_key) = auth.openai_api_key {
        Ok(StoredAccount::new_api_key(account_name, api_key))
    } else if let Some(tokens) = auth.tokens {
        let claims = parse_chatgpt_id_token_claims(&tokens.id_token);

        Ok(StoredAccount::new_chatgpt(
            account_name,
            claims.email,
            claims.plan_type,
            claims.subscription_expires_at,
            tokens.id_token,
            tokens.access_token,
            tokens.refresh_token,
            claims.account_id.or(tokens.account_id),
        ))
    } else {
        anyhow::bail!("auth.json contains neither API key nor tokens");
    }
}

/// Read the current auth.json file if it exists
pub fn read_current_auth() -> Result<Option<AuthDotJson>> {
    let path = get_codex_auth_file()?;

    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read auth.json: {}", path.display()))?;

    let auth: AuthDotJson = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse auth.json: {}", path.display()))?;

    Ok(Some(auth))
}

/// Check if there is an active Codex login
pub fn has_active_login() -> Result<bool> {
    match read_current_auth()? {
        Some(auth) => Ok(auth.openai_api_key.is_some() || auth.tokens.is_some()),
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_journal_store_delta, clear_pending_auth_sync, config_hash, config_write_required,
        merge_codex_config, read_config_backup, recovered_managed_state,
        strip_codex_config_overlay, validate_codex_config, write_config_backup, FileSnapshot,
    };
    use crate::types::{
        AccountsStore, CodexConfigManagedState, CodexConfigTransitionJournal, StoredAccount,
    };
    use toml_edit::DocumentMut;

    #[test]
    fn api_config_overlays_provider_settings_and_preserves_local_sections() {
        let normal = r#"
model = "normal-model"

[features]
multi_agent = true

[projects."D:/work/project"]
trust_level = "trusted"

[mcp_servers.local]
command = "local-server"
"#;
        let api = r#"
model_provider = "Proxy"
model = "api-model"

[model_providers.Proxy]
base_url = "https://example.com"
wire_api = "responses"

[features]
goals = true
"#;

        let merged = merge_codex_config(Some(normal), api).expect("config should merge");
        let document = merged
            .parse::<DocumentMut>()
            .expect("merged output should remain valid TOML");

        assert_eq!(document["model"].as_str(), Some("api-model"));
        assert_eq!(document["model_provider"].as_str(), Some("Proxy"));
        assert_eq!(document["features"]["multi_agent"].as_bool(), Some(true));
        assert_eq!(document["features"]["goals"].as_bool(), Some(true));
        assert_eq!(
            document["projects"]["D:/work/project"]["trust_level"].as_str(),
            Some("trusted")
        );
        assert_eq!(
            document["mcp_servers"]["local"]["command"].as_str(),
            Some("local-server")
        );
        assert_eq!(
            document["model_providers"]["Proxy"]["base_url"].as_str(),
            Some("https://example.com")
        );
    }

    #[test]
    fn rejects_invalid_api_config_before_it_is_saved() {
        assert!(validate_codex_config("model = [").is_err());
    }

    #[test]
    fn removing_api_overlay_preserves_projects_added_during_api_mode() {
        let baseline = r#"model = "normal"
[features]
multi_agent = true

[projects."D:/existing"]
trust_level = "trusted"
"#;
        let overlay = r#"model = "api"
[features]
goals = true
"#;
        let mut current = merge_codex_config(Some(baseline), overlay).expect("merge should work");
        current.push_str("\n[projects.\"D:/added-during-api\"]\ntrust_level = \"trusted\"\n");

        let restored = strip_codex_config_overlay(Some(baseline), Some(&current), overlay)
            .expect("overlay should be removable");
        let document = restored
            .parse::<DocumentMut>()
            .expect("result should be valid");

        assert_eq!(document["model"].as_str(), Some("normal"));
        assert_eq!(document["features"]["multi_agent"].as_bool(), Some(true));
        assert!(document["features"].get("goals").is_none());
        assert_eq!(
            document["projects"]["D:/added-during-api"]["trust_level"].as_str(),
            Some("trusted")
        );
    }

    #[test]
    fn interrupted_config_transition_recovers_only_known_states() {
        let before = CodexConfigManagedState {
            backup_captured: false,
            backup_existed: false,
            backup_home: None,
            active_overlay: None,
        };
        let after = CodexConfigManagedState {
            backup_captured: true,
            backup_existed: true,
            backup_home: Some("home".into()),
            active_overlay: Some("model = \"api\"".into()),
        };
        let journal = CodexConfigTransitionJournal {
            home: "home".into(),
            target_account_id: Some("target".into()),
            target_account: None,
            remove_account_id: None,
            before_config_hash: config_hash(Some("model = \"normal\"")),
            after_config_hash: config_hash(Some("model = \"api\"")),
            before,
            after,
        };

        assert!(
            !recovered_managed_state(&journal, &config_hash(Some("model = \"normal\"")))
                .expect("before state should recover")
                .backup_captured
        );
        assert!(
            recovered_managed_state(&journal, &config_hash(Some("model = \"api\"")))
                .expect("after state should recover")
                .backup_captured
        );
        assert!(
            recovered_managed_state(&journal, &config_hash(Some("model = \"edited\""))).is_err()
        );
    }

    #[test]
    fn regular_switch_does_not_plan_a_config_rewrite() {
        let config = "[projects.\"D:/work\"]\ntrust_level = \"trusted\"\n";
        assert!(!config_write_required(None, None));
        assert!(!config_write_required(Some(config), Some(config)));
        assert!(config_write_required(
            Some(config),
            Some("model = \"api\"\n")
        ));
    }

    #[test]
    fn no_op_transition_recovery_allows_external_config_edits() {
        let state = CodexConfigManagedState {
            backup_captured: false,
            backup_existed: false,
            backup_home: None,
            active_overlay: None,
        };
        let unchanged_hash = config_hash(Some("[projects.old]\ntrust_level = \"trusted\"\n"));
        let journal = CodexConfigTransitionJournal {
            home: "home".into(),
            target_account_id: Some("target".into()),
            target_account: None,
            remove_account_id: None,
            before_config_hash: unchanged_hash.clone(),
            after_config_hash: unchanged_hash,
            before: state.clone(),
            after: state,
        };

        assert!(recovered_managed_state(
            &journal,
            &config_hash(Some("[projects.new]\ntrust_level = \"trusted\"\n"))
        )
        .is_ok());
    }

    #[test]
    fn auth_only_finalize_does_not_clear_config_transition() {
        let state = CodexConfigManagedState {
            backup_captured: false,
            backup_existed: false,
            backup_home: None,
            active_overlay: None,
        };
        let mut store = AccountsStore {
            pending_auth_sync_account_id: Some("account".into()),
            codex_config_transition: Some(CodexConfigTransitionJournal {
                home: "home".into(),
                target_account_id: Some("other".into()),
                target_account: None,
                remove_account_id: None,
                before_config_hash: None,
                after_config_hash: None,
                before: state.clone(),
                after: state,
            }),
            ..AccountsStore::default()
        };

        assert!(clear_pending_auth_sync(&mut store, "account", "home"));
        assert!(store.pending_auth_sync_account_id.is_none());
        assert!(store.codex_config_transition.is_some());

        store.pending_auth_sync_account_id = Some("account".into());
        store.pending_auth_sync_home = Some("home-a".into());
        assert!(!clear_pending_auth_sync(&mut store, "account", "home-b"));
        assert_eq!(store.pending_auth_sync_home.as_deref(), Some("home-a"));
    }

    #[test]
    fn recovery_applies_target_snapshot_and_deletion_idempotently() {
        let removed = StoredAccount::new_api_key("Removed".into(), "sk-old".into());
        let mut target = StoredAccount::new_api_key("Replacement".into(), "sk-new".into());
        target.codex_config = Some("model = \"new\"".into());
        let state = CodexConfigManagedState {
            backup_captured: false,
            backup_existed: false,
            backup_home: None,
            active_overlay: None,
        };
        let journal = CodexConfigTransitionJournal {
            home: "home".into(),
            target_account_id: Some(target.id.clone()),
            target_account: Some(target.clone()),
            remove_account_id: Some(removed.id.clone()),
            before_config_hash: None,
            after_config_hash: None,
            before: state.clone(),
            after: state,
        };
        let mut store = AccountsStore::default();
        store.accounts.push(removed.clone());
        store.active_account_id = Some(removed.id.clone());

        apply_journal_store_delta(&mut store, &journal).expect("delta should apply");
        apply_journal_store_delta(&mut store, &journal).expect("delta should be idempotent");

        assert_eq!(store.active_account_id.as_deref(), Some(target.id.as_str()));
        assert_eq!(store.active_account_home.as_deref(), Some("home"));
        assert_eq!(store.accounts.len(), 1);
        assert_eq!(store.accounts[0].codex_config, target.codex_config);
    }

    #[test]
    fn stores_original_config_in_a_dedicated_backup_file() {
        let directory = std::env::temp_dir().join(format!(
            "codex-switcher-config-backup-{}",
            uuid::Uuid::new_v4()
        ));
        let backup_path = directory.join("config.toml.backup");
        let original = "[projects.\"D:/work\"]\ntrust_level = \"trusted\"\n";

        write_config_backup(&backup_path, original).expect("backup should be written");
        let restored = read_config_backup(&backup_path, &AccountsStore::default())
            .expect("backup should be readable");

        assert_eq!(restored, original);
        std::fs::remove_dir_all(directory).expect("temporary backup directory should be removed");
    }

    #[test]
    fn file_snapshot_restores_previous_contents_after_failure() {
        let directory = std::env::temp_dir().join(format!(
            "codex-switcher-file-rollback-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).expect("temporary directory should be created");
        let path = directory.join("auth.json");
        std::fs::write(&path, b"before").expect("original should be written");

        let snapshot = FileSnapshot::capture(&path).expect("snapshot should be captured");
        std::fs::write(&path, b"after").expect("replacement should be written");
        snapshot.restore().expect("snapshot should be restored");

        assert_eq!(
            std::fs::read(&path).expect("file should be readable"),
            b"before"
        );
        std::fs::remove_dir_all(directory).expect("temporary directory should be removed");
    }
}
