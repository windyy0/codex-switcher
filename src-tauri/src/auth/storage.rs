//! Account storage module - manages reading and writing accounts.json

use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use anyhow::{Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

use crate::types::{AccountsStore, AppSettings, AuthData, StoredAccount};

pub(crate) const MAX_CONSUMED_REFRESH_TOKEN_HASHES: usize = 512;

pub fn refresh_token_fingerprint(refresh_token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(refresh_token.as_bytes()))
}

pub fn has_consumed_refresh_token(store: &AccountsStore, refresh_token: &str) -> bool {
    let fingerprint = refresh_token_fingerprint(refresh_token);
    store
        .consumed_refresh_token_hashes
        .iter()
        .any(|existing| existing == &fingerprint)
}

pub fn remember_consumed_refresh_token(store: &mut AccountsStore, refresh_token: &str) {
    let fingerprint = refresh_token_fingerprint(refresh_token);
    if store
        .consumed_refresh_token_hashes
        .iter()
        .any(|existing| existing == &fingerprint)
    {
        return;
    }

    store.consumed_refresh_token_hashes.push(fingerprint);
    let overflow = store
        .consumed_refresh_token_hashes
        .len()
        .saturating_sub(MAX_CONSUMED_REFRESH_TOKEN_HASHES);
    if overflow > 0 {
        store.consumed_refresh_token_hashes.drain(..overflow);
    }
}

pub(crate) fn merge_consumed_refresh_token_hashes(
    store: &mut AccountsStore,
    fingerprints: impl IntoIterator<Item = String>,
) {
    for fingerprint in fingerprints {
        if store.consumed_refresh_token_hashes.len() >= MAX_CONSUMED_REFRESH_TOKEN_HASHES {
            break;
        }
        if !store
            .consumed_refresh_token_hashes
            .iter()
            .any(|existing| existing == &fingerprint)
        {
            store.consumed_refresh_token_hashes.push(fingerprint);
        }
    }
}

/// Get the path to the codex-switcher config directory
pub fn get_config_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".codex-switcher"))
}

/// Get the path to accounts.json
pub fn get_accounts_file() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("accounts.json"))
}

pub fn get_settings_file() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("settings.json"))
}

/// Load the accounts store from disk
pub fn load_accounts() -> Result<AccountsStore> {
    let path = get_accounts_file()?;

    if !path.exists() {
        return Ok(AccountsStore::default());
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read accounts file: {}", path.display()))?;

    let store: AccountsStore = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse accounts file: {}", path.display()))?;

    Ok(store)
}

pub fn load_app_settings() -> Result<AppSettings> {
    let path = get_settings_file()?;

    if !path.exists() {
        return Ok(AppSettings::default());
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read settings file: {}", path.display()))?;

    let settings: AppSettings = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse settings file: {}", path.display()))?;

    Ok(settings)
}

pub fn save_app_settings(settings: &AppSettings) -> Result<()> {
    let path = get_settings_file()?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
    }

    let content = serde_json::to_string_pretty(settings).context("Failed to serialize settings")?;
    fs::write(&path, content)
        .with_context(|| format!("Failed to write settings file: {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(&path, perms)?;
    }

    Ok(())
}

/// Save the accounts store to disk
pub fn save_accounts(store: &AccountsStore) -> Result<()> {
    let path = get_accounts_file()?;

    // Ensure the config directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
    }

    let content =
        serde_json::to_string_pretty(store).context("Failed to serialize accounts store")?;

    write_file_atomic(&path, content.as_bytes())
        .with_context(|| format!("Failed to write accounts file: {}", path.display()))?;

    // Set restrictive permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(&path, perms)?;
    }

    Ok(())
}

/// Serialize remote refresh-token exchanges with exports and imports across
/// desktop/web processes. The lock file itself is never replaced or removed.
pub fn lock_credential_exchange() -> Result<fs::File> {
    let config_dir = get_config_dir()?;
    fs::create_dir_all(&config_dir)?;
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(config_dir.join("credentials.lock"))?;
    file.lock()?;
    Ok(file)
}

pub async fn lock_credential_exchange_async() -> Result<fs::File> {
    tokio::task::spawn_blocking(lock_credential_exchange)
        .await
        .context("Credential lock task failed")?
}

/// Write a file through a same-directory temporary file and atomic rename so
/// readers never observe a truncated JSON/TOML/auth document.
pub(crate) fn write_file_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    // Preserve dotfile symlinks: atomically replace their target rather than
    // replacing the link itself with a regular file.
    let resolved_path = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Some(
            fs::canonicalize(path)
                .with_context(|| format!("Failed to resolve symlink: {}", path.display()))?,
        ),
        _ => None,
    };
    let target = resolved_path.as_deref().unwrap_or(path);

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .context("Target file name is not valid UTF-8")?;
    let temp_path = target.with_file_name(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));
    let previous_permissions = fs::metadata(target)
        .ok()
        .map(|metadata| metadata.permissions());

    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        if let Some(permissions) = previous_permissions {
            fs::set_permissions(&temp_path, permissions)?;
        }
        #[cfg(unix)]
        if !target.exists() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o600))?;
        }
        replace_file_atomic(&temp_path, target)?;
        #[cfg(unix)]
        if let Some(parent) = target.parent() {
            fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();

    if result.is_err() && temp_path.exists() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

#[cfg(windows)]
fn replace_file_atomic(source: &Path, target: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, ReplaceFileW, MOVEFILE_WRITE_THROUGH, REPLACE_FILE_FLAGS,
    };

    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let target_wide = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();

    if target.exists() {
        unsafe {
            ReplaceFileW(
                PCWSTR(target_wide.as_ptr()),
                PCWSTR(source_wide.as_ptr()),
                PCWSTR::null(),
                REPLACE_FILE_FLAGS(0),
                None,
                None,
            )
        }
        .with_context(|| format!("Failed to atomically replace {}", target.display()))?;
    } else {
        unsafe {
            MoveFileExW(
                PCWSTR(source_wide.as_ptr()),
                PCWSTR(target_wide.as_ptr()),
                MOVEFILE_WRITE_THROUGH,
            )
        }
        .with_context(|| format!("Failed to atomically create {}", target.display()))?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file_atomic(source: &Path, target: &Path) -> Result<()> {
    fs::rename(source, target)?;
    Ok(())
}

/// Add a new account to the store
pub fn add_account(account: StoredAccount) -> Result<StoredAccount> {
    let mut store = load_accounts()?;

    // Check for duplicate names
    if store.accounts.iter().any(|a| a.name == account.name) {
        anyhow::bail!("An account with name '{}' already exists", account.name);
    }
    if has_duplicate_chatgpt_credentials(&store, &account) {
        anyhow::bail!("This ChatGPT account has already been added");
    }

    let account_clone = account.clone();
    store.accounts.push(account);

    save_accounts(&store)?;
    Ok(account_clone)
}

/// Remove an account by ID
pub fn remove_account(account_id: &str) -> Result<()> {
    let mut store = load_accounts()?;

    let initial_len = store.accounts.len();
    store.accounts.retain(|a| a.id != account_id);

    if store.accounts.len() == initial_len {
        anyhow::bail!("Account not found: {account_id}");
    }

    // A replacement is activated only by the higher-level guarded switch.
    if store.active_account_id.as_deref() == Some(account_id) {
        store.active_account_id = None;
        store.active_account_home = None;
        store.pending_auth_sync_account_id = None;
        store.pending_auth_sync_home = None;
    }

    save_accounts(&store)?;
    Ok(())
}

/// Update the active account ID
pub fn set_active_account(account_id: &str) -> Result<()> {
    let mut store = load_accounts()?;

    // Verify the account exists
    if !store.accounts.iter().any(|a| a.id == account_id) {
        anyhow::bail!("Account not found: {account_id}");
    }

    store.active_account_id = Some(account_id.to_string());
    store.active_account_home = Some(super::switcher::get_codex_home_identity()?);
    save_accounts(&store)?;
    Ok(())
}

/// Get an account by ID
pub fn get_account(account_id: &str) -> Result<Option<StoredAccount>> {
    let store = load_accounts()?;
    Ok(store.accounts.into_iter().find(|a| a.id == account_id))
}

/// Get the currently active account
pub fn get_active_account() -> Result<Option<StoredAccount>> {
    let store = load_accounts()?;
    let active_id = match &store.active_account_id {
        Some(id) => id,
        None => return Ok(None),
    };
    Ok(store.accounts.into_iter().find(|a| a.id == *active_id))
}

/// Update an account's last_used_at timestamp
pub fn touch_account(account_id: &str) -> Result<()> {
    let mut store = load_accounts()?;

    if let Some(account) = store.accounts.iter_mut().find(|a| a.id == account_id) {
        account.last_used_at = Some(chrono::Utc::now());
        save_accounts(&store)?;
    }

    Ok(())
}

pub(crate) fn has_duplicate_chatgpt_credentials(
    store: &AccountsStore,
    account: &StoredAccount,
) -> bool {
    let AuthData::ChatGPT {
        refresh_token,
        account_id,
        ..
    } = &account.auth_data
    else {
        return false;
    };
    has_consumed_refresh_token(store, refresh_token)
        || store
            .accounts
            .iter()
            .any(|existing| match &existing.auth_data {
                AuthData::ChatGPT {
                    refresh_token: existing_token,
                    account_id: existing_account_id,
                    ..
                } => {
                    existing_token == refresh_token
                        || account_id.as_deref().is_some_and(|candidate| {
                            !candidate.is_empty()
                                && existing_account_id.as_deref() == Some(candidate)
                        })
                }
                AuthData::ApiKey { .. } => false,
            })
}

/// Store an optional Codex config.toml template for an API-key account.
pub fn set_account_codex_config(account_id: &str, config: Option<String>) -> Result<StoredAccount> {
    let mut store = load_accounts()?;
    let account = store
        .accounts
        .iter_mut()
        .find(|account| account.id == account_id)
        .context("Account not found")?;

    if account.auth_mode != crate::types::AuthMode::ApiKey {
        anyhow::bail!("Per-account Codex configuration is only available for API key accounts");
    }

    account.codex_config = config.filter(|value| !value.trim().is_empty());
    let updated = account.clone();
    save_accounts(&store)?;
    Ok(updated)
}

pub fn get_account_codex_config(account_id: &str) -> Result<Option<String>> {
    let account = get_account(account_id)?.context("Account not found")?;
    if account.auth_mode != crate::types::AuthMode::ApiKey {
        anyhow::bail!("Per-account Codex configuration is only available for API key accounts");
    }
    Ok(account.codex_config)
}

/// Update an account's metadata (name, email, plan_type, subscription expiry)
pub fn update_account_metadata(
    account_id: &str,
    name: Option<String>,
    email: Option<String>,
    plan_type: Option<String>,
    subscription_expires_at: Option<Option<DateTime<Utc>>>,
) -> Result<StoredAccount> {
    let mut store = load_accounts()?;

    // Check for duplicate names first (if renaming)
    if let Some(ref new_name) = name {
        if store
            .accounts
            .iter()
            .any(|a| a.id != account_id && a.name == *new_name)
        {
            anyhow::bail!("An account with name '{new_name}' already exists");
        }
    }

    // Now find and update the account
    let account = store
        .accounts
        .iter_mut()
        .find(|a| a.id == account_id)
        .context("Account not found")?;

    if let Some(new_name) = name {
        account.name = new_name;
    }

    if email.is_some() {
        account.email = email;
    }

    if plan_type.is_some() {
        account.plan_type = plan_type;
    }

    if let Some(subscription_expires_at) = subscription_expires_at {
        account.subscription_expires_at = subscription_expires_at;
    }

    let updated = account.clone();
    save_accounts(&store)?;
    Ok(updated)
}

/// Update ChatGPT OAuth tokens for an account and return the updated account.
pub fn update_account_chatgpt_tokens(
    account_id: &str,
    id_token: String,
    access_token: String,
    refresh_token: String,
    chatgpt_account_id: Option<String>,
    email: Option<String>,
    plan_type: Option<String>,
    subscription_expires_at: Option<DateTime<Utc>>,
) -> Result<StoredAccount> {
    let mut store = load_accounts()?;

    let account = store
        .accounts
        .iter_mut()
        .find(|a| a.id == account_id)
        .context("Account not found")?;

    match &mut account.auth_data {
        AuthData::ChatGPT {
            id_token: stored_id_token,
            access_token: stored_access_token,
            refresh_token: stored_refresh_token,
            account_id: stored_account_id,
        } => {
            *stored_id_token = id_token;
            *stored_access_token = access_token;
            *stored_refresh_token = refresh_token;
            if let Some(new_account_id) = chatgpt_account_id {
                *stored_account_id = Some(new_account_id);
            }
        }
        AuthData::ApiKey { .. } => {
            anyhow::bail!("Cannot update OAuth tokens for an API key account");
        }
    }

    if let Some(new_email) = email {
        account.email = Some(new_email);
    }

    if let Some(new_plan_type) = plan_type {
        account.plan_type = Some(new_plan_type);
    }

    if let Some(subscription_expires_at) = subscription_expires_at {
        account.subscription_expires_at = Some(subscription_expires_at);
    }

    let updated = account.clone();
    save_accounts(&store)?;
    Ok(updated)
}

/// Get the list of masked account IDs
pub fn get_masked_account_ids() -> Result<Vec<String>> {
    let store = load_accounts()?;
    Ok(store.masked_account_ids.clone())
}

/// Set the list of masked account IDs
pub fn set_masked_account_ids(ids: Vec<String>) -> Result<()> {
    let mut store = load_accounts()?;
    store.masked_account_ids = ids;
    save_accounts(&store)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        has_consumed_refresh_token, has_duplicate_chatgpt_credentials,
        remember_consumed_refresh_token, write_file_atomic, MAX_CONSUMED_REFRESH_TOKEN_HASHES,
    };
    use crate::types::{AccountsStore, StoredAccount};
    use std::fs;

    #[test]
    fn atomic_write_replaces_an_existing_file() {
        let directory = std::env::temp_dir().join(format!(
            "codex-switcher-atomic-write-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).expect("temp directory should be created");
        let path = directory.join("existing.json");
        fs::write(&path, b"old").expect("fixture should be written");

        write_file_atomic(&path, b"new").expect("existing file should be atomically replaced");

        assert_eq!(fs::read(&path).expect("result should be readable"), b"new");
        fs::remove_dir_all(directory).expect("temp directory should be removed");
    }

    #[test]
    fn duplicate_chatgpt_credentials_are_rejected_independent_of_name() {
        let original = StoredAccount::new_chatgpt(
            "Original".into(),
            None,
            None,
            None,
            "id".into(),
            "access".into(),
            "shared-refresh".into(),
            None,
        );
        let duplicate = StoredAccount::new_chatgpt(
            "Different name".into(),
            None,
            None,
            None,
            "other-id".into(),
            "other-access".into(),
            "shared-refresh".into(),
            None,
        );
        let mut store = AccountsStore::default();
        store.accounts.push(original);

        assert!(has_duplicate_chatgpt_credentials(&store, &duplicate));
    }

    #[test]
    fn duplicate_chatgpt_identity_is_rejected_after_token_rotation() {
        let original = StoredAccount::new_chatgpt(
            "Original".into(),
            None,
            None,
            None,
            "id".into(),
            "access".into(),
            "refresh-a".into(),
            Some("chatgpt-account".into()),
        );
        let duplicate = StoredAccount::new_chatgpt(
            "Different name".into(),
            None,
            None,
            None,
            "other-id".into(),
            "other-access".into(),
            "refresh-b".into(),
            Some("chatgpt-account".into()),
        );
        let mut store = AccountsStore::default();
        store.accounts.push(original);

        assert!(has_duplicate_chatgpt_credentials(&store, &duplicate));
    }

    #[test]
    fn consumed_refresh_tokens_are_deduplicated_and_bounded() {
        let mut store = AccountsStore::default();
        remember_consumed_refresh_token(&mut store, "already-used");
        remember_consumed_refresh_token(&mut store, "already-used");
        assert_eq!(store.consumed_refresh_token_hashes.len(), 1);
        assert!(has_consumed_refresh_token(&store, "already-used"));

        for index in 0..=MAX_CONSUMED_REFRESH_TOKEN_HASHES {
            remember_consumed_refresh_token(&mut store, &format!("token-{index}"));
        }
        assert_eq!(
            store.consumed_refresh_token_hashes.len(),
            MAX_CONSUMED_REFRESH_TOKEN_HASHES
        );
        assert!(!has_consumed_refresh_token(&store, "already-used"));
        assert!(has_consumed_refresh_token(
            &store,
            &format!("token-{MAX_CONSUMED_REFRESH_TOKEN_HASHES}")
        ));
    }

    #[test]
    fn consumed_chatgpt_credentials_are_rejected_after_rotation() {
        let candidate = StoredAccount::new_chatgpt(
            "Imported again".into(),
            None,
            None,
            None,
            "id".into(),
            "access".into(),
            "old-refresh".into(),
            None,
        );
        let mut store = AccountsStore::default();
        remember_consumed_refresh_token(&mut store, "old-refresh");

        assert!(has_duplicate_chatgpt_credentials(&store, &candidate));
    }
}
