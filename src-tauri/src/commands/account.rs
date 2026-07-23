//! Account management Tauri commands

use crate::auth::{
    add_account, clear_codex_account_files_removing, create_chatgpt_account_from_refresh_token,
    finalize_account_auth_sync, finalize_codex_transition, get_account, get_account_codex_config,
    get_codex_home_identity, has_consumed_refresh_token, has_duplicate_chatgpt_credentials,
    import_from_auth_json, import_from_auth_json_contents, load_accounts,
    lock_credential_exchange_async, merge_consumed_refresh_token_hashes,
    recover_pending_account_transition, remember_consumed_refresh_token, remove_account,
    restore_codex_state, save_accounts, set_account_codex_config, snapshot_codex_state,
    switch_to_account, switch_to_account_removing, sync_account_auth_file_at_home,
    validate_codex_config, MAX_CONSUMED_REFRESH_TOKEN_HASHES,
};
use crate::types::{
    AccountInfo, AccountsStore, AuthData, AuthMode, ImportAccountsSummary, StoredAccount,
};

use super::process::ensure_codex_not_running;

use anyhow::Context;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    XChaCha20Poly1305, XNonce,
};
use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use pbkdf2::pbkdf2_hmac;
use rand::RngCore;
use sha2::Sha256;
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use tokio::sync::Mutex as AsyncMutex;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

const SLIM_EXPORT_PREFIX: &str = "css1.";
const SLIM_FORMAT_VERSION: u8 = 2;
const MIN_SUPPORTED_SLIM_FORMAT_VERSION: u8 = 1;
const SLIM_AUTH_API_KEY: u8 = 0;
const SLIM_AUTH_CHATGPT: u8 = 1;

const FULL_FILE_MAGIC: &[u8; 4] = b"CSWF";
const FULL_FILE_VERSION: u8 = 2;
const MIN_SUPPORTED_FULL_FILE_VERSION: u8 = 1;
const FULL_SALT_LEN: usize = 16;
const FULL_NONCE_LEN: usize = 24;
const FULL_KDF_ITERATIONS: u32 = 210_000;
const FULL_PRESET_PASSPHRASE: &str = "gT7kQ9mV2xN4pL8sR1dH6zW3cB5yF0uJ_aE7nK2tP9vM4rX1";

const MAX_IMPORT_JSON_BYTES: u64 = 2 * 1024 * 1024;
const MAX_IMPORT_FILE_BYTES: u64 = 8 * 1024 * 1024;
static ACCOUNT_TRANSITION_LOCK: Mutex<()> = Mutex::new(());
static SLIM_IMPORT_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

pub struct AccountTransitionGuard {
    _local: MutexGuard<'static, ()>,
    _cross_process: fs::File,
}

pub fn lock_account_transition() -> Result<AccountTransitionGuard, String> {
    let local = ACCOUNT_TRANSITION_LOCK
        .lock()
        .map_err(|_| "Account transition lock is poisoned".to_string())?;
    let config_dir = crate::auth::get_config_dir().map_err(|error| error.to_string())?;
    fs::create_dir_all(&config_dir).map_err(|error| error.to_string())?;
    let cross_process = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(config_dir.join("state.lock"))
        .map_err(|error| error.to_string())?;
    cross_process.lock().map_err(|error| error.to_string())?;
    let guard = AccountTransitionGuard {
        _local: local,
        _cross_process: cross_process,
    };
    // The desktop UI and codex-web may outlive one another. Recover while both
    // process-local and cross-process state locks are held so every subsequent
    // read-modify-write starts from a fully committed account transition.
    recover_pending_account_transition().map_err(|error| error.to_string())?;
    Ok(guard)
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct SlimPayload {
    #[serde(rename = "v")]
    version: u8,
    #[serde(rename = "a", skip_serializing_if = "Option::is_none")]
    active_name: Option<String>,
    #[serde(rename = "c")]
    accounts: Vec<SlimAccountPayload>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SlimAccountPayload {
    #[serde(rename = "n")]
    name: String,
    #[serde(rename = "d", default, skip_serializing_if = "is_false")]
    disabled: bool,
    #[serde(rename = "t")]
    auth_type: u8,
    #[serde(rename = "k", skip_serializing_if = "Option::is_none")]
    api_key: Option<String>,
    #[serde(rename = "r", skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(rename = "g", default, skip_serializing_if = "Option::is_none")]
    codex_config: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn slim_entry_exists(store: &AccountsStore, entry: &SlimAccountPayload) -> bool {
    if store
        .accounts
        .iter()
        .any(|account| account.name == entry.name)
    {
        return true;
    }

    let Some(source) = entry.refresh_token.as_deref() else {
        return false;
    };
    has_consumed_refresh_token(store, source)
        || store.accounts.iter().any(|account| {
            matches!(
                &account.auth_data,
                AuthData::ChatGPT { refresh_token, .. } if refresh_token == source
            )
        })
}

fn chatgpt_account_id(account: &StoredAccount) -> Option<&str> {
    match &account.auth_data {
        AuthData::ChatGPT {
            account_id: Some(account_id),
            ..
        } if !account_id.is_empty() => Some(account_id),
        _ => None,
    }
}

fn refresh_token_was_rotated(account: &StoredAccount, source: Option<&str>) -> bool {
    source.is_some_and(|source| {
        matches!(
            &account.auth_data,
            AuthData::ChatGPT { refresh_token, .. } if refresh_token != source
        )
    })
}

/// List all accounts with their info
#[tauri::command]
pub async fn list_accounts() -> Result<Vec<AccountInfo>, String> {
    let _guard = lock_account_transition()?;
    let store = load_accounts().map_err(|e| e.to_string())?;
    let active_id = store.active_account_id.as_deref();

    let accounts: Vec<AccountInfo> = store
        .accounts
        .iter()
        .map(|a| AccountInfo::from_stored(a, active_id))
        .collect();

    Ok(accounts)
}

/// Get the currently active account
#[tauri::command]
pub async fn get_active_account_info() -> Result<Option<AccountInfo>, String> {
    let _guard = lock_account_transition()?;
    let store = load_accounts().map_err(|e| e.to_string())?;
    let active_id = store.active_account_id.as_deref();

    if let Some(active) = store
        .accounts
        .iter()
        .find(|account| Some(account.id.as_str()) == active_id)
    {
        Ok(Some(AccountInfo::from_stored(active, active_id)))
    } else {
        Ok(None)
    }
}

/// Locate the auth.json used by the local Codex installation.
#[tauri::command]
pub async fn detect_local_auth_json() -> Result<Option<String>, String> {
    let mut candidates = Vec::new();

    if let Some(codex_home) = std::env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
        candidates.push(PathBuf::from(codex_home).join("auth.json"));
    }

    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".codex").join("auth.json"));
    }

    Ok(candidates
        .into_iter()
        .find(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned()))
}

/// Add an account from an auth.json file
#[tauri::command]
pub async fn add_account_from_file(path: String, name: String) -> Result<AccountInfo, String> {
    // Import from the file
    let account = import_from_auth_json(&path, name).map_err(|e| e.to_string())?;
    let _credential_exchange = lock_credential_exchange_async()
        .await
        .map_err(|e| e.to_string())?;
    let _guard = lock_account_transition()?;

    // Add to storage
    let stored = add_account(account).map_err(|e| e.to_string())?;

    let store = load_accounts().map_err(|e| e.to_string())?;
    let active_id = store.active_account_id.as_deref();

    Ok(AccountInfo::from_stored(&stored, active_id))
}

/// Add an account from uploaded auth.json contents.
pub async fn add_account_from_auth_json_text(
    name: String,
    contents: String,
) -> Result<AccountInfo, String> {
    let account = import_from_auth_json_contents(&contents, name).map_err(|e| e.to_string())?;
    let _credential_exchange = lock_credential_exchange_async()
        .await
        .map_err(|e| e.to_string())?;
    let _guard = lock_account_transition()?;
    let stored = add_account(account).map_err(|e| e.to_string())?;

    let store = load_accounts().map_err(|e| e.to_string())?;
    let active_id = store.active_account_id.as_deref();

    Ok(AccountInfo::from_stored(&stored, active_id))
}

/// Add an API-key account without requiring the user to create auth.json first.
#[tauri::command]
pub async fn add_api_account(
    name: String,
    api_key: String,
    config: Option<String>,
) -> Result<AccountInfo, String> {
    let _guard = lock_account_transition()?;
    if name.trim().is_empty() {
        return Err("Account name is required".to_string());
    }
    if api_key.trim().is_empty() {
        return Err("API key is required".to_string());
    }

    let config = config.filter(|value| !value.trim().is_empty());
    if let Some(contents) = config.as_deref() {
        validate_codex_config(contents).map_err(|e| format!("{e:#}"))?;
    }

    let mut account =
        StoredAccount::new_api_key(name.trim().to_string(), api_key.trim().to_string());
    account.codex_config = config;
    let stored = add_account(account).map_err(|e| e.to_string())?;
    let store = load_accounts().map_err(|e| e.to_string())?;
    Ok(AccountInfo::from_stored(
        &stored,
        store.active_account_id.as_deref(),
    ))
}

/// Switch to a different account
#[tauri::command]
pub async fn switch_account(account_id: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || switch_account_by_id(&account_id))
        .await
        .map_err(|error| format!("Account switch task failed: {error}"))?
}

pub fn switch_account_by_id(account_id: &str) -> Result<(), String> {
    let _guard = lock_account_transition()?;
    switch_account_by_id_unlocked(account_id)
}

pub(crate) fn switch_account_by_id_unlocked(account_id: &str) -> Result<(), String> {
    let store = load_accounts().map_err(|e| e.to_string())?;

    // Find the account
    let account = store
        .accounts
        .iter()
        .find(|a| a.id == account_id)
        .cloned()
        .ok_or_else(|| format!("Account not found: {account_id}"))?;
    if account.disabled {
        return Err("Account is disabled".to_string());
    }
    ensure_codex_not_running()?;
    let transition_snapshot = snapshot_codex_state().map_err(|e| e.to_string())?;

    // Write to ~/.codex/auth.json
    switch_to_account(&account).map_err(|e| e.to_string())?;

    // Reload after file switching because config backup state may have changed.
    let mut updated_store = load_accounts().map_err(|e| e.to_string())?;
    updated_store.active_account_id = Some(account_id.to_string());
    if let Some(updated_account) = updated_store
        .accounts
        .iter_mut()
        .find(|candidate| candidate.id == account_id)
    {
        updated_account.last_used_at = Some(chrono::Utc::now());
    }
    if let Err(error) = save_accounts(&updated_store) {
        let rollback = restore_codex_state(&transition_snapshot);
        return Err(match rollback {
            Ok(()) => error.to_string(),
            Err(rollback_error) => format!("{error}; rollback failed: {rollback_error:#}"),
        });
    }
    finalize_codex_transition().map_err(|e| e.to_string())?;

    // Restart Antigravity background process if it is running
    // This allows it to pick up the new authorization file seamlessly
    restart_antigravity_if_running();

    Ok(())
}

/// Remove an account
#[tauri::command]
pub async fn delete_account(account_id: String) -> Result<(), String> {
    let _credential_exchange = lock_credential_exchange_async()
        .await
        .map_err(|e| e.to_string())?;
    let _guard = lock_account_transition()?;
    let store = load_accounts().map_err(|e| e.to_string())?;
    let _deleted = store
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .cloned()
        .ok_or_else(|| format!("Account not found: {account_id}"))?;

    if store.active_account_id.as_deref() != Some(account_id.as_str()) {
        remove_account(&account_id).map_err(|e| e.to_string())?;
        return Ok(());
    }
    let current_home = get_codex_home_identity().map_err(|error| error.to_string())?;
    if let Some(active_home) = store
        .active_account_home
        .as_deref()
        .filter(|active_home| *active_home != current_home)
    {
        return Err(format!(
            "The active account belongs to a different CODEX_HOME ({active_home})"
        ));
    }

    ensure_codex_not_running()?;
    let transition_snapshot = snapshot_codex_state().map_err(|e| e.to_string())?;
    let replacement = store
        .accounts
        .iter()
        .find(|account| account.id != account_id && !account.disabled)
        .cloned();
    match &replacement {
        Some(account) => {
            switch_to_account_removing(account, &account_id).map_err(|e| e.to_string())?
        }
        None => clear_codex_account_files_removing(&account_id).map_err(|e| e.to_string())?,
    }

    let mut updated_store = load_accounts().map_err(|e| e.to_string())?;
    updated_store
        .accounts
        .retain(|account| account.id != account_id);
    updated_store.active_account_id = replacement.as_ref().map(|account| account.id.clone());
    if let Some(replacement_id) = updated_store.active_account_id.clone() {
        if let Some(account) = updated_store
            .accounts
            .iter_mut()
            .find(|account| account.id == replacement_id)
        {
            account.last_used_at = Some(chrono::Utc::now());
        }
    }
    if let Err(error) = save_accounts(&updated_store) {
        let rollback = restore_codex_state(&transition_snapshot);
        return Err(match rollback {
            Ok(()) => error.to_string(),
            Err(rollback_error) => format!("{error}; rollback failed: {rollback_error:#}"),
        });
    }
    finalize_codex_transition().map_err(|e| e.to_string())?;
    restart_antigravity_if_running();
    Ok(())
}

/// Rename an account
#[tauri::command]
pub async fn rename_account(account_id: String, new_name: String) -> Result<(), String> {
    let _guard = lock_account_transition()?;
    crate::auth::storage::update_account_metadata(&account_id, Some(new_name), None, None, None)
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Enable or disable an account without deleting its stored credentials.
#[tauri::command]
pub async fn set_account_disabled(
    account_id: String,
    disabled: bool,
) -> Result<AccountInfo, String> {
    let _guard = lock_account_transition()?;
    let mut store = load_accounts().map_err(|e| e.to_string())?;
    let account = store
        .accounts
        .iter_mut()
        .find(|account| account.id == account_id)
        .ok_or_else(|| format!("Account not found: {account_id}"))?;
    account.disabled = disabled;
    save_accounts(&store).map_err(|e| e.to_string())?;
    crate::commands::usage::clear_usage_cache(&account_id);

    let active_id = store.active_account_id.as_deref();
    let account = store
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .expect("updated account should still exist");
    Ok(AccountInfo::from_stored(account, active_id))
}

/// Save the config.toml to apply whenever an API-key account is switched to.
#[tauri::command]
pub async fn set_api_account_config(
    account_id: String,
    config: Option<String>,
) -> Result<(), String> {
    let _guard = lock_account_transition()?;
    if let Some(contents) = config.as_deref().filter(|value| !value.trim().is_empty()) {
        validate_codex_config(contents).map_err(|e| format!("{e:#}"))?;
    }
    let original = get_account(&account_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Account not found: {account_id}"))?;
    let normalized_config = config.filter(|value| !value.trim().is_empty());
    let mut updated = original.clone();
    if updated.auth_mode != crate::types::AuthMode::ApiKey {
        return Err(
            "Per-account Codex configuration is only available for API key accounts".into(),
        );
    }
    updated.codex_config = normalized_config.clone();

    // Editing the active account should take effect immediately, without a
    // second switch.  Credentials are re-written as part of the same sync.
    let store = load_accounts().map_err(|e| e.to_string())?;
    if store.active_account_id.as_deref() == Some(account_id.as_str()) {
        let current_home = get_codex_home_identity().map_err(|error| error.to_string())?;
        if let Some(active_home) = store
            .active_account_home
            .as_deref()
            .filter(|active_home| *active_home != current_home)
        {
            return Err(format!(
                "The active account belongs to a different CODEX_HOME ({active_home})"
            ));
        }
        ensure_codex_not_running()?;
        let transition_snapshot = snapshot_codex_state().map_err(|e| e.to_string())?;
        switch_to_account(&updated).map_err(|e| e.to_string())?;
        if let Err(error) = set_account_codex_config(&account_id, normalized_config) {
            let rollback = restore_codex_state(&transition_snapshot);
            return Err(match rollback {
                Ok(()) => error.to_string(),
                Err(rollback_error) => format!("{error}; rollback failed: {rollback_error:#}"),
            });
        }
        finalize_codex_transition().map_err(|e| e.to_string())?;
        restart_antigravity_if_running();
    } else {
        set_account_codex_config(&account_id, normalized_config).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn get_api_account_config(account_id: String) -> Result<Option<String>, String> {
    get_account_codex_config(&account_id).map_err(|e| e.to_string())
}

/// Export minimal account config as a compact text string.
/// For ChatGPT accounts, only refresh token is exported.
#[tauri::command]
pub async fn export_accounts_slim_text() -> Result<String, String> {
    let _credential_exchange = lock_credential_exchange_async()
        .await
        .map_err(|e| e.to_string())?;
    let _guard = lock_account_transition()?;
    let store = load_accounts().map_err(|e| e.to_string())?;
    encode_slim_payload_from_store(&store).map_err(|e| e.to_string())
}

/// Import minimal account config from a compact text string, skipping existing accounts.
#[tauri::command]
pub async fn import_accounts_slim_text(payload: String) -> Result<ImportAccountsSummary, String> {
    let _import_guard = SLIM_IMPORT_LOCK.lock().await;
    let slim_payload = decode_slim_payload(&payload).map_err(|e| format!("{e:#}"))?;
    let total_in_payload = slim_payload.accounts.len();
    let mut imported_count = 0usize;

    for entry in slim_payload.accounts {
        let source_refresh_token = entry.refresh_token.clone();
        let credential_exchange = lock_credential_exchange_async()
            .await
            .map_err(|e| e.to_string())?;

        // Recheck only after obtaining the cross-process credential lock. A
        // different process may have exchanged this rotating source token while
        // this importer was waiting.
        {
            let _guard = lock_account_transition()?;
            let current = load_accounts().map_err(|e| e.to_string())?;
            if slim_entry_exists(&current, &entry) {
                continue;
            }
        }

        let mut restored = restore_slim_accounts(vec![entry])
            .await
            .map_err(|e| {
                format!(
                    "{e:#}\n{imported_count} account(s) were already imported safely. Hint: Slim import needs network access to refresh ChatGPT tokens."
                )
            })?;
        let account = restored
            .pop()
            .context("Slim account restoration returned no account")
            .map_err(|e| e.to_string())?;
        let source_was_rotated =
            refresh_token_was_rotated(&account, source_refresh_token.as_deref());

        // A ChatGPT refresh token may have rotated at the server. Persist the
        // resulting account immediately, before attempting the next network
        // refresh, so a later failure cannot discard the new credential.
        let _guard = lock_account_transition()?;
        let mut current = load_accounts().map_err(|e| e.to_string())?;
        let restored_account_id = chatgpt_account_id(&account).map(str::to_owned);
        if let Some(existing_index) = current.accounts.iter().position(|existing| {
            let source_matches = matches!(
                (&existing.auth_data, source_refresh_token.as_deref()),
                (AuthData::ChatGPT { refresh_token, .. }, Some(source)) if refresh_token == source
            );
            let identity_matches = restored_account_id
                .as_deref()
                .is_some_and(|account_id| chatgpt_account_id(existing) == Some(account_id));
            source_matches || identity_matches
        }) {
            // Preserve the existing local identity/name, but keep the result of
            // the exchange: it may have rotated the same server-side account.
            let existing = &mut current.accounts[existing_index];
            existing.auth_data = account.auth_data;
            existing.email = account.email;
            existing.plan_type = account.plan_type;
            existing.subscription_expires_at = account.subscription_expires_at;
            let updated = existing.clone();
            let is_active = current.active_account_id.as_deref() == Some(updated.id.as_str());
            let active_home = if is_active {
                Some(match &current.active_account_home {
                    Some(home) => home.clone(),
                    None => get_codex_home_identity().map_err(|error| error.to_string())?,
                })
            } else {
                None
            };
            if let Some(active_home) = active_home.as_deref() {
                current.active_account_home = Some(active_home.to_string());
                current.pending_auth_sync_account_id = Some(updated.id.clone());
                current.pending_auth_sync_home = Some(active_home.to_string());
            }
            if source_was_rotated {
                let source = source_refresh_token
                    .as_deref()
                    .expect("rotated ChatGPT source token should exist");
                remember_consumed_refresh_token(&mut current, source);
            }
            save_accounts(&current).map_err(|e| e.to_string())?;
            if let Some(active_home) = active_home.as_deref() {
                sync_account_auth_file_at_home(&updated, std::path::Path::new(active_home))
                    .map_err(|e| e.to_string())?;
                finalize_account_auth_sync(&updated.id, active_home).map_err(|e| e.to_string())?;
            }
            continue;
        }
        let mut account = account;
        if current
            .accounts
            .iter()
            .any(|existing| existing.name == account.name)
        {
            let base = account.name.clone();
            let mut suffix = 2usize;
            while current
                .accounts
                .iter()
                .any(|existing| existing.name == format!("{base} ({suffix})"))
            {
                suffix += 1;
            }
            account.name = format!("{base} ({suffix})");
        }
        if source_was_rotated {
            let source = source_refresh_token
                .as_deref()
                .expect("rotated ChatGPT source token should exist");
            remember_consumed_refresh_token(&mut current, source);
        }
        current.accounts.push(account);
        save_accounts(&current).map_err(|e| e.to_string())?;
        imported_count += 1;
        drop(credential_exchange);
    }

    Ok(ImportAccountsSummary {
        total_in_payload,
        imported_count,
        skipped_count: total_in_payload.saturating_sub(imported_count),
    })
}

/// Export full account config as an encrypted file.
#[tauri::command]
pub async fn export_accounts_full_encrypted_file(path: String) -> Result<(), String> {
    let _credential_exchange = lock_credential_exchange_async()
        .await
        .map_err(|e| e.to_string())?;
    let _guard = lock_account_transition()?;
    let store = load_accounts().map_err(|e| e.to_string())?;
    let encrypted =
        encode_full_encrypted_store(&store, FULL_PRESET_PASSPHRASE).map_err(|e| e.to_string())?;
    write_encrypted_file(&path, &encrypted).map_err(|e| e.to_string())?;
    Ok(())
}

/// Export full account config as encrypted bytes for browser clients.
pub async fn export_accounts_full_encrypted_bytes() -> Result<Vec<u8>, String> {
    let _credential_exchange = lock_credential_exchange_async()
        .await
        .map_err(|e| e.to_string())?;
    let _guard = lock_account_transition()?;
    let store = load_accounts().map_err(|e| e.to_string())?;
    encode_full_encrypted_store(&store, FULL_PRESET_PASSPHRASE).map_err(|e| e.to_string())
}

/// Import full account config from an encrypted file, skipping existing accounts.
#[tauri::command]
pub async fn import_accounts_full_encrypted_file(
    path: String,
) -> Result<ImportAccountsSummary, String> {
    let encrypted = read_encrypted_file(&path).map_err(|e| e.to_string())?;
    let imported = decode_full_encrypted_store(&encrypted, FULL_PRESET_PASSPHRASE)
        .map_err(|e| e.to_string())?;
    validate_imported_store(&imported).map_err(|e| e.to_string())?;

    let _credential_exchange = lock_credential_exchange_async()
        .await
        .map_err(|e| e.to_string())?;
    let _guard = lock_account_transition()?;
    let current = load_accounts().map_err(|e| e.to_string())?;
    let (merged, summary) = merge_accounts_store(current, imported);
    save_accounts(&merged).map_err(|e| e.to_string())?;
    Ok(summary)
}

/// Import full account config from encrypted bytes uploaded through the browser UI.
pub async fn import_accounts_full_encrypted_bytes(
    bytes: Vec<u8>,
) -> Result<ImportAccountsSummary, String> {
    let imported =
        decode_full_encrypted_store(&bytes, FULL_PRESET_PASSPHRASE).map_err(|e| e.to_string())?;
    validate_imported_store(&imported).map_err(|e| e.to_string())?;

    let _credential_exchange = lock_credential_exchange_async()
        .await
        .map_err(|e| e.to_string())?;
    let _guard = lock_account_transition()?;
    let current = load_accounts().map_err(|e| e.to_string())?;
    let (merged, summary) = merge_accounts_store(current, imported);
    save_accounts(&merged).map_err(|e| e.to_string())?;
    Ok(summary)
}

/// Find all running Antigravity codex assistant processes
fn restart_antigravity_if_running() {
    if let Ok(pids) = find_antigravity_processes() {
        for pid in pids {
            #[cfg(unix)]
            {
                let _ = std::process::Command::new("kill")
                    .arg("-9")
                    .arg(pid.to_string())
                    .output();
            }
            #[cfg(windows)]
            {
                let _ = std::process::Command::new("taskkill")
                    .args(["/F", "/PID", &pid.to_string()])
                    .output();
            }
        }
    }
}

fn find_antigravity_processes() -> anyhow::Result<Vec<u32>> {
    let mut pids = Vec::new();

    #[cfg(unix)]
    {
        // Use ps with custom format to get the pid and full command line
        let output = std::process::Command::new("ps")
            .args(["-eo", "pid,command"])
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines().skip(1) {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if let Some((pid_str, command)) = line.split_once(' ') {
                let pid_str = pid_str.trim();
                let command = command.trim();

                // Antigravity processes have a specific path format
                let is_antigravity = (command.contains(".antigravity/extensions/openai.chatgpt")
                    || command.contains(".vscode/extensions/openai.chatgpt"))
                    && (command.ends_with("codex app-server --analytics-default-enabled")
                        || command.contains("/codex app-server"));

                if is_antigravity {
                    if let Ok(pid) = pid_str.parse::<u32>() {
                        pids.push(pid);
                    }
                }
            }
        }
    }

    #[cfg(windows)]
    {
        // Use tasklist on Windows
        // For Windows we might need a more precise WMI query to get command line args,
        // but for now we look for codex.exe PIDs and verify they're not ours
        let output = std::process::Command::new("tasklist")
            .creation_flags(CREATE_NO_WINDOW)
            .args(["/FI", "IMAGENAME eq codex.exe", "/FO", "CSV", "/NH"])
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() > 1 {
                let name = parts[0].trim_matches('"').to_lowercase();
                if name == "codex.exe" {
                    let pid_str = parts[1].trim_matches('"');
                    if let Ok(pid) = pid_str.parse::<u32>() {
                        pids.push(pid);
                    }
                }
            }
        }
    }

    Ok(pids)
}

fn encode_slim_payload_from_store(store: &AccountsStore) -> anyhow::Result<String> {
    let active_name = store.active_account_id.as_ref().and_then(|active_id| {
        store
            .accounts
            .iter()
            .find(|account| account.id == *active_id)
            .map(|account| account.name.clone())
    });

    let slim_accounts: Vec<SlimAccountPayload> = store
        .accounts
        .iter()
        .map(|account| match &account.auth_data {
            AuthData::ApiKey { key } => SlimAccountPayload {
                name: account.name.clone(),
                disabled: account.disabled,
                auth_type: SLIM_AUTH_API_KEY,
                api_key: Some(key.clone()),
                refresh_token: None,
                codex_config: account.codex_config.clone(),
            },
            AuthData::ChatGPT { refresh_token, .. } => SlimAccountPayload {
                name: account.name.clone(),
                disabled: account.disabled,
                auth_type: SLIM_AUTH_CHATGPT,
                api_key: None,
                refresh_token: Some(refresh_token.clone()),
                codex_config: None,
            },
        })
        .collect();

    let version = if slim_accounts
        .iter()
        .any(|account| account.codex_config.is_some())
    {
        SLIM_FORMAT_VERSION
    } else {
        MIN_SUPPORTED_SLIM_FORMAT_VERSION
    };
    let payload = SlimPayload {
        version,
        active_name,
        accounts: slim_accounts,
    };

    let json = serde_json::to_vec(&payload).context("Failed to serialize slim payload")?;
    let compressed = compress_bytes(&json).context("Failed to compress slim payload")?;

    Ok(format!(
        "{SLIM_EXPORT_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(compressed)
    ))
}

fn decode_slim_payload(payload: &str) -> anyhow::Result<SlimPayload> {
    let normalized: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
    if normalized.is_empty() {
        anyhow::bail!("Import string is empty");
    }

    let encoded = normalized
        .strip_prefix(SLIM_EXPORT_PREFIX)
        .unwrap_or(&normalized);

    let compressed = URL_SAFE_NO_PAD
        .decode(encoded)
        .context("Invalid slim import string (base64 decode failed)")?;

    let decompressed = decompress_bytes_with_limit(&compressed, MAX_IMPORT_JSON_BYTES)
        .context("Invalid slim import string (decompression failed)")?;

    let parsed: SlimPayload = serde_json::from_slice(&decompressed)
        .context("Invalid slim import string (JSON parse failed)")?;

    validate_slim_payload(&parsed)?;
    Ok(parsed)
}

fn validate_slim_payload(payload: &SlimPayload) -> anyhow::Result<()> {
    if !(MIN_SUPPORTED_SLIM_FORMAT_VERSION..=SLIM_FORMAT_VERSION).contains(&payload.version) {
        anyhow::bail!("Unsupported slim payload version: {}", payload.version);
    }

    let mut names = HashSet::new();
    let mut chatgpt_refresh_tokens = HashSet::new();

    for account in &payload.accounts {
        if account.name.trim().is_empty() {
            anyhow::bail!("Slim import contains an account with empty name");
        }

        if !names.insert(account.name.clone()) {
            anyhow::bail!(
                "Slim import contains duplicate account name: {}",
                account.name
            );
        }

        match account.auth_type {
            SLIM_AUTH_API_KEY => {
                if account
                    .api_key
                    .as_ref()
                    .map_or(true, |key| key.trim().is_empty())
                {
                    anyhow::bail!("API key is missing for account {}", account.name);
                }
                if let Some(config) = account.codex_config.as_deref() {
                    if config.trim().is_empty() {
                        anyhow::bail!("API config is empty for account {}", account.name);
                    }
                    validate_codex_config(config).with_context(|| {
                        format!("Invalid API config for account {}", account.name)
                    })?;
                }
                if account.refresh_token.is_some() {
                    anyhow::bail!(
                        "API key account {} contains unexpected ChatGPT credentials",
                        account.name
                    );
                }
            }
            SLIM_AUTH_CHATGPT => {
                let refresh_token = account
                    .refresh_token
                    .as_ref()
                    .filter(|token| !token.trim().is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("Refresh token is missing for account {}", account.name)
                    })?;
                if !chatgpt_refresh_tokens.insert(refresh_token.clone()) {
                    anyhow::bail!(
                        "Slim import contains duplicate ChatGPT credentials for account {}",
                        account.name
                    );
                }
                if account.api_key.is_some() || account.codex_config.is_some() {
                    anyhow::bail!(
                        "ChatGPT account {} contains unexpected API-key configuration",
                        account.name
                    );
                }
            }
            _ => {
                anyhow::bail!(
                    "Unsupported auth type {} for account {}",
                    account.auth_type,
                    account.name
                );
            }
        }
    }

    if let Some(active_name) = &payload.active_name {
        if !names.contains(active_name) {
            anyhow::bail!("Slim import references missing active account: {active_name}");
        }
    }

    Ok(())
}

async fn restore_slim_accounts(
    entries: Vec<SlimAccountPayload>,
) -> anyhow::Result<Vec<StoredAccount>> {
    let mut restored = Vec::with_capacity(entries.len());
    for entry in entries {
        let SlimAccountPayload {
            name: account_name,
            disabled,
            auth_type,
            api_key,
            refresh_token,
            codex_config,
        } = entry;
        let mut account = match auth_type {
            SLIM_AUTH_API_KEY => {
                let mut account = StoredAccount::new_api_key(
                    account_name.clone(),
                    api_key.context("API key payload is missing")?,
                );
                account.codex_config = codex_config;
                account
            }
            SLIM_AUTH_CHATGPT => {
                let refresh_token = refresh_token.context("Refresh token payload is missing")?;
                create_chatgpt_account_from_refresh_token(account_name.clone(), refresh_token)
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to restore ChatGPT account `{account_name}` from refresh token"
                        )
                    })?
            }
            _ => anyhow::bail!("Unsupported auth type in slim payload"),
        };
        account.disabled = disabled;
        restored.push(account);
    }
    Ok(restored)
}

fn encode_full_encrypted_store(store: &AccountsStore, passphrase: &str) -> anyhow::Result<Vec<u8>> {
    // Config backup state describes the current machine's in-progress switch,
    // not portable account data. In particular, never embed a legacy inline
    // copy of the user's original config.toml in a full account export.
    let mut export_store = store.clone();
    export_store.codex_config_backup_captured = false;
    export_store.codex_config_backup = None;
    export_store.codex_config_backup_existed = false;
    export_store.codex_config_backup_home = None;
    export_store.codex_config_active_overlay = None;
    export_store.codex_config_transition = None;
    export_store.active_account_home = None;
    export_store.pending_auth_sync_account_id = None;
    export_store.pending_auth_sync_home = None;
    let json = serde_json::to_vec(&export_store).context("Failed to serialize account store")?;
    let compressed = compress_bytes(&json).context("Failed to compress account store")?;

    let mut salt = [0u8; FULL_SALT_LEN];
    rand::rng().fill_bytes(&mut salt);

    let mut nonce = [0u8; FULL_NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce);

    let key = derive_encryption_key(passphrase, &salt);
    let cipher = XChaCha20Poly1305::new((&key).into());
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce), compressed.as_slice())
        .map_err(|_| anyhow::anyhow!("Failed to encrypt account store"))?;

    let mut out = Vec::with_capacity(4 + 1 + FULL_SALT_LEN + FULL_NONCE_LEN + ciphertext.len());
    out.extend_from_slice(FULL_FILE_MAGIC);
    out.push(FULL_FILE_VERSION);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);

    Ok(out)
}

fn decode_full_encrypted_store(
    file_bytes: &[u8],
    passphrase: &str,
) -> anyhow::Result<AccountsStore> {
    if file_bytes.len() as u64 > MAX_IMPORT_FILE_BYTES {
        anyhow::bail!("Encrypted file is too large");
    }

    let header_len = 4 + 1 + FULL_SALT_LEN + FULL_NONCE_LEN;
    if file_bytes.len() <= header_len {
        anyhow::bail!("Encrypted file is invalid or truncated");
    }

    if &file_bytes[..4] != FULL_FILE_MAGIC {
        anyhow::bail!("Encrypted file header is invalid");
    }

    let version = file_bytes[4];
    if !(MIN_SUPPORTED_FULL_FILE_VERSION..=FULL_FILE_VERSION).contains(&version) {
        anyhow::bail!("Unsupported encrypted file version: {version}");
    }

    let salt_start = 5;
    let nonce_start = salt_start + FULL_SALT_LEN;
    let ciphertext_start = nonce_start + FULL_NONCE_LEN;

    let salt = &file_bytes[salt_start..nonce_start];
    let nonce = &file_bytes[nonce_start..ciphertext_start];
    let ciphertext = &file_bytes[ciphertext_start..];

    let key = derive_encryption_key(passphrase, salt);
    let cipher = XChaCha20Poly1305::new((&key).into());
    let compressed = cipher
        .decrypt(XNonce::from_slice(nonce), ciphertext)
        .map_err(|_| {
            anyhow::anyhow!("Failed to decrypt file (wrong passphrase or corrupted file)")
        })?;

    let json = decompress_bytes_with_limit(&compressed, MAX_IMPORT_JSON_BYTES)
        .context("Failed to decompress decrypted payload")?;

    let store: AccountsStore =
        serde_json::from_slice(&json).context("Failed to parse decrypted account payload")?;

    Ok(store)
}

fn derive_encryption_key(passphrase: &str, salt: &[u8]) -> [u8; 32] {
    let mut key = [0u8; 32];
    pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), salt, FULL_KDF_ITERATIONS, &mut key);
    key
}

fn compress_bytes(input: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(input)?;
    encoder.finish().context("Failed to finalize compression")
}

fn decompress_bytes_with_limit(input: &[u8], max_bytes: u64) -> anyhow::Result<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(input);
    let mut limited = decoder.by_ref().take(max_bytes + 1);
    let mut decompressed = Vec::new();
    limited.read_to_end(&mut decompressed)?;

    if decompressed.len() as u64 > max_bytes {
        anyhow::bail!("Import data is too large");
    }

    Ok(decompressed)
}

fn write_encrypted_file(path: &str, bytes: &[u8]) -> anyhow::Result<()> {
    fs::write(path, bytes).with_context(|| format!("Failed to write file: {path}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("Failed to set file permissions: {path}"))?;
    }

    Ok(())
}

fn read_encrypted_file(path: &str) -> anyhow::Result<Vec<u8>> {
    let metadata =
        fs::metadata(path).with_context(|| format!("Failed to read file metadata: {path}"))?;
    if metadata.len() > MAX_IMPORT_FILE_BYTES {
        anyhow::bail!("Encrypted file is too large");
    }

    fs::read(path).with_context(|| format!("Failed to read file: {path}"))
}

fn validate_imported_store(store: &AccountsStore) -> anyhow::Result<()> {
    let mut ids = HashSet::new();
    let mut names = HashSet::new();

    if store.consumed_refresh_token_hashes.len() > MAX_CONSUMED_REFRESH_TOKEN_HASHES {
        anyhow::bail!("Import contains too many consumed refresh-token fingerprints");
    }
    let mut fingerprints = HashSet::new();
    for fingerprint in &store.consumed_refresh_token_hashes {
        let decoded = URL_SAFE_NO_PAD
            .decode(fingerprint)
            .context("Import contains an invalid refresh-token fingerprint")?;
        if decoded.len() != 32 || !fingerprints.insert(fingerprint) {
            anyhow::bail!("Import contains an invalid refresh-token fingerprint");
        }
    }

    for account in &store.accounts {
        if account.id.trim().is_empty() {
            anyhow::bail!("Import contains an account with empty id");
        }
        if account.name.trim().is_empty() {
            anyhow::bail!("Import contains an account with empty name");
        }
        if !ids.insert(account.id.clone()) {
            anyhow::bail!("Import contains duplicate account id: {}", account.id);
        }
        if !names.insert(account.name.clone()) {
            anyhow::bail!("Import contains duplicate account name: {}", account.name);
        }

        match (&account.auth_mode, &account.auth_data) {
            (AuthMode::ApiKey, AuthData::ApiKey { key }) => {
                if key.trim().is_empty() {
                    anyhow::bail!(
                        "Import contains an empty API key for account {}",
                        account.name
                    );
                }
                if let Some(config) = account.codex_config.as_deref() {
                    if config.trim().is_empty() {
                        anyhow::bail!(
                            "Import contains an empty API config for account {}",
                            account.name
                        );
                    }
                    validate_codex_config(config).with_context(|| {
                        format!("Invalid API config for account {}", account.name)
                    })?;
                }
            }
            (AuthMode::ChatGPT, AuthData::ChatGPT { refresh_token, .. }) => {
                if refresh_token.trim().is_empty() {
                    anyhow::bail!(
                        "Import contains an empty refresh token for account {}",
                        account.name
                    );
                }
                if account.codex_config.is_some() {
                    anyhow::bail!(
                        "Import contains an API config for non-API account {}",
                        account.name
                    );
                }
            }
            _ => {
                anyhow::bail!(
                    "Import contains mismatched authentication mode and data for account {}",
                    account.name
                );
            }
        }
    }

    if let Some(active_id) = &store.active_account_id {
        if !ids.contains(active_id) {
            anyhow::bail!("Import references a missing active account: {active_id}");
        }
    }

    Ok(())
}

fn merge_accounts_store(
    mut current: AccountsStore,
    imported: AccountsStore,
) -> (AccountsStore, ImportAccountsSummary) {
    let imported_version = imported.version;
    let total_in_payload = imported.accounts.len();
    let imported_consumed_refresh_token_hashes = imported.consumed_refresh_token_hashes;
    let mut imported_count = 0usize;
    let mut existing_ids: HashSet<String> = current.accounts.iter().map(|a| a.id.clone()).collect();
    let mut existing_names: HashSet<String> =
        current.accounts.iter().map(|a| a.name.clone()).collect();
    for account in imported.accounts {
        let duplicate_chatgpt_credentials = has_duplicate_chatgpt_credentials(&current, &account);
        if existing_ids.contains(&account.id)
            || existing_names.contains(&account.name)
            || duplicate_chatgpt_credentials
        {
            continue;
        }
        existing_ids.insert(account.id.clone());
        existing_names.insert(account.name.clone());
        current.accounts.push(account);
        imported_count += 1;
    }

    current.version = current.version.max(imported_version).max(1);
    merge_consumed_refresh_token_hashes(&mut current, imported_consumed_refresh_token_hashes);

    let current_active_is_valid = current
        .active_account_id
        .as_ref()
        .is_some_and(|id| current.accounts.iter().any(|a| &a.id == id));

    if !current_active_is_valid {
        current.active_account_id = None;
        current.active_account_home = None;
    }

    (
        current,
        ImportAccountsSummary {
            total_in_payload,
            imported_count,
            skipped_count: total_in_payload.saturating_sub(imported_count),
        },
    )
}

/// Get the list of masked account IDs
#[tauri::command]
pub async fn get_masked_account_ids() -> Result<Vec<String>, String> {
    crate::auth::storage::get_masked_account_ids().map_err(|e| e.to_string())
}

/// Set the list of masked account IDs
#[tauri::command]
pub async fn set_masked_account_ids(ids: Vec<String>) -> Result<(), String> {
    let _guard = lock_account_transition()?;
    crate::auth::storage::set_masked_account_ids(ids).map_err(|e| e.to_string())
}

#[cfg(test)]
mod api_switching_tests {
    use super::{
        decode_full_encrypted_store, decode_slim_payload, encode_full_encrypted_store,
        encode_slim_payload_from_store, merge_accounts_store, refresh_token_was_rotated,
        slim_entry_exists, validate_imported_store, SlimAccountPayload, SlimPayload,
        FULL_FILE_VERSION, FULL_PRESET_PASSPHRASE, SLIM_AUTH_API_KEY, SLIM_AUTH_CHATGPT,
    };
    use crate::auth::{has_consumed_refresh_token, remember_consumed_refresh_token};
    use crate::types::{AccountsStore, AuthMode, StoredAccount};

    #[test]
    fn slim_round_trip_preserves_api_config() {
        let mut account = StoredAccount::new_api_key("Proxy".to_string(), "sk-test".to_string());
        account.codex_config = Some(
            "model_provider = \"Proxy\"\n[model_providers.Proxy]\nbase_url = \"https://example.com\"\n"
                .to_string(),
        );
        let mut store = AccountsStore::default();
        store.accounts.push(account);

        let encoded = encode_slim_payload_from_store(&store).expect("slim export should work");
        let decoded = decode_slim_payload(&encoded).expect("slim import should work");

        assert_eq!(decoded.version, 2);
        assert_eq!(
            decoded.accounts[0].codex_config.as_deref(),
            store.accounts[0].codex_config.as_deref()
        );
    }

    #[test]
    fn legacy_slim_payload_without_api_config_remains_supported() {
        let payload = SlimPayload {
            version: 1,
            active_name: None,
            accounts: vec![SlimAccountPayload {
                name: "Legacy".to_string(),
                disabled: false,
                auth_type: SLIM_AUTH_API_KEY,
                api_key: Some("sk-test".to_string()),
                refresh_token: None,
                codex_config: None,
            }],
        };

        super::validate_slim_payload(&payload).expect("v1 payload should remain valid");
    }

    #[test]
    fn slim_export_without_api_config_remains_version_one() {
        let mut store = AccountsStore::default();
        store.accounts.push(StoredAccount::new_api_key(
            "Legacy".into(),
            "sk-test".into(),
        ));

        let encoded = encode_slim_payload_from_store(&store).expect("slim export should work");
        let decoded = decode_slim_payload(&encoded).expect("slim import should work");

        assert_eq!(decoded.version, 1);
    }

    #[test]
    fn slim_round_trip_preserves_disabled_accounts() {
        let mut account = StoredAccount::new_api_key("Archived".into(), "sk-test".into());
        account.disabled = true;
        let mut store = AccountsStore::default();
        store.accounts.push(account);

        let encoded = encode_slim_payload_from_store(&store).expect("slim export should work");
        let decoded = decode_slim_payload(&encoded).expect("slim import should work");

        assert!(decoded.accounts[0].disabled);
    }

    #[test]
    fn slim_import_skips_a_source_token_consumed_by_another_process() {
        let entry = SlimAccountPayload {
            name: "Different local name".into(),
            disabled: false,
            auth_type: SLIM_AUTH_CHATGPT,
            api_key: None,
            refresh_token: Some("rotated-source".into()),
            codex_config: None,
        };
        let mut store = AccountsStore::default();
        remember_consumed_refresh_token(&mut store, "rotated-source");

        assert!(slim_entry_exists(&store, &entry));
    }

    #[test]
    fn full_import_preserves_consumed_refresh_token_history() {
        let current = AccountsStore::default();
        let mut imported = AccountsStore::default();
        remember_consumed_refresh_token(&mut imported, "old-rotated-token");

        let (merged, _) = merge_accounts_store(current, imported);

        assert!(has_consumed_refresh_token(&merged, "old-rotated-token"));
    }

    #[test]
    fn slim_import_marks_only_a_refresh_token_that_really_rotated() {
        let unchanged = StoredAccount::new_chatgpt(
            "Unchanged".into(),
            None,
            None,
            None,
            "id".into(),
            "new-access".into(),
            "source".into(),
            None,
        );
        let rotated = StoredAccount::new_chatgpt(
            "Rotated".into(),
            None,
            None,
            None,
            "id".into(),
            "new-access".into(),
            "replacement".into(),
            None,
        );

        assert!(!refresh_token_was_rotated(&unchanged, Some("source")));
        assert!(refresh_token_was_rotated(&rotated, Some("source")));
    }

    #[test]
    fn full_export_is_versioned_and_excludes_local_backup_state() {
        let mut account = StoredAccount::new_api_key("Proxy".into(), "sk-test".into());
        account.codex_config = Some("model = \"proxy\"".into());
        account.disabled = true;
        let mut store = AccountsStore::default();
        store.accounts.push(account);
        store.codex_config_backup_captured = true;
        store.codex_config_backup_existed = true;
        store.codex_config_backup = Some("private local config".into());
        store.codex_config_backup_home = Some("C:/Users/example/.codex".into());
        store.codex_config_active_overlay = Some("model = \"proxy\"".into());
        store.active_account_home = Some("C:/Users/example/.codex".into());
        store.pending_auth_sync_account_id = Some("pending".into());
        store.pending_auth_sync_home = Some("C:/Users/example/.codex".into());
        remember_consumed_refresh_token(&mut store, "rotated-token");

        let encoded = encode_full_encrypted_store(&store, FULL_PRESET_PASSPHRASE)
            .expect("full export should work");
        assert_eq!(encoded[4], FULL_FILE_VERSION);

        let decoded = decode_full_encrypted_store(&encoded, FULL_PRESET_PASSPHRASE)
            .expect("full import should work");
        assert_eq!(
            decoded.accounts[0].codex_config.as_deref(),
            Some("model = \"proxy\"")
        );
        assert!(decoded.accounts[0].disabled);
        assert!(!decoded.codex_config_backup_captured);
        assert!(!decoded.codex_config_backup_existed);
        assert!(decoded.codex_config_backup.is_none());
        assert!(decoded.codex_config_backup_home.is_none());
        assert!(decoded.codex_config_active_overlay.is_none());
        assert!(decoded.active_account_home.is_none());
        assert!(decoded.pending_auth_sync_account_id.is_none());
        assert!(decoded.pending_auth_sync_home.is_none());
        assert!(has_consumed_refresh_token(&decoded, "rotated-token"));

        // Version 1 used the same encrypted payload framing. Current builds
        // must continue accepting those existing backup files.
        let mut legacy = encoded;
        legacy[4] = 1;
        decode_full_encrypted_store(&legacy, FULL_PRESET_PASSPHRASE)
            .expect("version 1 full backup should remain supported");
    }

    #[test]
    fn full_import_rejects_invalid_api_account_invariants() {
        let mut mismatched = StoredAccount::new_api_key("Mismatch".into(), "sk-test".into());
        mismatched.auth_mode = AuthMode::ChatGPT;
        let mut store = AccountsStore::default();
        store.accounts.push(mismatched);
        assert!(validate_imported_store(&store).is_err());

        let mut chatgpt = StoredAccount::new_chatgpt(
            "ChatGPT".into(),
            None,
            None,
            None,
            "id-token".into(),
            "access-token".into(),
            "refresh-token".into(),
            None,
        );
        chatgpt.codex_config = Some("model = \"proxy\"".into());
        store.accounts.clear();
        store.accounts.push(chatgpt);
        assert!(validate_imported_store(&store).is_err());

        let mut invalid_config = StoredAccount::new_api_key("Invalid".into(), "sk-test".into());
        invalid_config.codex_config = Some("model = [".into());
        store.accounts.clear();
        store.accounts.push(invalid_config);
        assert!(validate_imported_store(&store).is_err());
    }

    #[test]
    fn imported_accounts_do_not_claim_to_be_active_before_files_are_applied() {
        let current = AccountsStore::default();
        let account = StoredAccount::new_api_key("Imported".into(), "sk-test".into());
        let mut imported = AccountsStore::default();
        imported.active_account_id = Some(account.id.clone());
        imported.accounts.push(account);

        let (merged, _) = merge_accounts_store(current, imported);
        assert_eq!(merged.accounts.len(), 1);
        assert!(merged.active_account_id.is_none());
    }
}
