//! OAuth login Tauri commands

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

use crate::account_health::classify_error_message;
use crate::auth::oauth_server::{start_oauth_login, wait_for_oauth_login, OAuthLoginResult};
use crate::auth::{
    add_account, finalize_account_auth_sync, get_codex_home_identity, load_accounts,
    lock_credential_exchange_async, record_account_health, remember_consumed_refresh_token,
    remove_account, save_accounts, sync_account_auth_file_at_home,
};
use crate::commands::account::{lock_account_transition, switch_account_by_id_unlocked};
use crate::types::{
    parse_chatgpt_id_token_claims, AccountHealthSource, AccountHealthStatus, AccountInfo, AuthData,
    AuthMode, OAuthLoginInfo, StoredAccount,
};

struct PendingOAuth {
    rx: Option<oneshot::Receiver<anyhow::Result<OAuthLoginResult>>>,
    cancelled: Arc<AtomicBool>,
    target_account_id: Option<String>,
}

// Global state for pending OAuth login
static PENDING_OAUTH: Mutex<Option<PendingOAuth>> = Mutex::new(None);
static OAUTH_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Start the OAuth login flow
#[tauri::command]
pub async fn start_login(
    account_name: String,
    target_account_id: Option<String>,
) -> Result<OAuthLoginInfo, String> {
    let generation = OAUTH_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let account_name = if let Some(target_account_id) = target_account_id.as_deref() {
        let store = load_accounts().map_err(|error| error.to_string())?;
        let target = store
            .accounts
            .iter()
            .find(|account| account.id == target_account_id)
            .ok_or_else(|| format!("Account not found: {target_account_id}"))?;
        if target.auth_mode != AuthMode::ChatGPT {
            return Err("Only ChatGPT accounts can be reauthorized".to_string());
        }
        target.name.clone()
    } else {
        account_name
    };

    // Cancel any previous pending flow so it does not keep the callback port occupied.
    if let Some(previous) = {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        pending.take()
    } {
        previous.cancelled.store(true, Ordering::Relaxed);
    }

    let (info, rx, cancelled) = start_oauth_login(account_name)
        .await
        .map_err(|e| e.to_string())?;

    // Store the receiver for later
    {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        // The user may cancel or start another flow while the callback server
        // is still starting. Check while holding the same mutex cancel_login
        // uses so this stale flow can never be published after cancellation.
        if OAUTH_GENERATION.load(Ordering::SeqCst) != generation {
            cancelled.store(true, Ordering::Relaxed);
            return Err("OAuth login was cancelled".to_string());
        }
        let previous = pending.replace(PendingOAuth {
            rx: Some(rx),
            cancelled,
            target_account_id,
        });
        if let Some(previous) = previous {
            previous.cancelled.store(true, Ordering::Relaxed);
        }
    }

    Ok(info)
}

/// Wait for the OAuth login to complete and add the account
#[tauri::command]
pub async fn complete_login() -> Result<AccountInfo, String> {
    // Leave the pending flow registered while awaiting the browser callback so
    // cancel_login and a replacement start_login can still invalidate it.
    let (rx, cancelled, target_account_id) = {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        let pending = pending
            .as_mut()
            .ok_or_else(|| "No pending OAuth login".to_string())?;
        let rx = pending
            .rx
            .take()
            .ok_or_else(|| "OAuth login completion is already being awaited".to_string())?;
        (
            rx,
            Arc::clone(&pending.cancelled),
            pending.target_account_id.clone(),
        )
    };

    let account = match wait_for_oauth_login(rx).await {
        Ok(account) => account,
        Err(error) => {
            if let Some(target_account_id) = target_account_id.as_deref() {
                persist_oauth_deactivation_if_present(target_account_id, &error.to_string());
            }
            clear_pending_oauth_if_current(&cancelled);
            return Err(error.to_string());
        }
    };

    // Serialize publishing newly issued credentials with refresh-token
    // rotation, Slim import, deletion, and credential snapshots.
    let _credential_exchange = match lock_credential_exchange_async().await {
        Ok(guard) => guard,
        Err(error) => {
            clear_pending_oauth_if_current(&cancelled);
            return Err(error.to_string());
        }
    };

    // Keep this guard until the synchronous add/switch transaction finishes.
    // Cancellation that wins before this point prevents any account changes;
    // cancellation after it is a no-op because login has already committed.
    let mut pending = PENDING_OAUTH.lock().unwrap();
    let is_current = pending
        .as_ref()
        .map(|current| Arc::ptr_eq(&current.cancelled, &cancelled))
        .unwrap_or(false);
    if !is_current || cancelled.load(Ordering::Relaxed) {
        if is_current {
            pending.take();
        }
        return Err("OAuth login was cancelled".to_string());
    }

    let result = (|| {
        // Add/replace credentials under the same transition lock so no
        // concurrent import or token refresh can overwrite the result.
        let _transition_guard = lock_account_transition()?;
        if let Some(target_account_id) = target_account_id.as_deref() {
            return replace_reauthorized_account(target_account_id, account);
        }

        let stored = add_account(account).map_err(|e| e.to_string())?;

        // Use the same guarded transition as manual account switching. The active
        // marker is committed only after auth.json/config.toml have been applied.
        if let Err(switch_error) = switch_account_by_id_unlocked(&stored.id) {
            let rollback = remove_account(&stored.id);
            return Err(match rollback {
                Ok(()) => switch_error,
                Err(rollback_error) => {
                    format!(
                        "{switch_error}; failed to remove the newly added account: {rollback_error:#}"
                    )
                }
            });
        }

        let store = load_accounts().map_err(|e| e.to_string())?;
        let active_id = store.active_account_id.as_deref();
        let switched = store
            .accounts
            .iter()
            .find(|account| account.id == stored.id)
            .ok_or_else(|| "The newly added account disappeared after switching".to_string())?;

        Ok(AccountInfo::from_stored(switched, active_id))
    })();
    pending.take();
    result
}

/// Stop a targeted OAuth flow and record an exact error shown by the provider page.
#[tauri::command]
pub async fn report_oauth_page_error(
    account_id: String,
    error_text: String,
) -> Result<AccountInfo, String> {
    let error_text = error_text.trim();
    if error_text.is_empty() {
        return Err(
            "Paste the authorization page error or choose a known error button".to_string(),
        );
    }

    let observation = classify_error_message(AccountHealthSource::OAuthUserReport, error_text);
    if !matches!(
        observation.status,
        AccountHealthStatus::AccountDeactivated | AccountHealthStatus::WorkspaceDeactivated
    ) {
        return Err(
            "The pasted text did not contain a supported account_deactivated or workspace_deactivated signal"
                .to_string(),
        );
    }

    let pending = {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        let current = pending
            .as_ref()
            .ok_or_else(|| "No pending OAuth login".to_string())?;
        if current.target_account_id.as_deref() != Some(account_id.as_str()) {
            return Err("The pending OAuth login belongs to a different account".to_string());
        }
        pending
            .take()
            .ok_or_else(|| "No pending OAuth login".to_string())?
    };
    pending.cancelled.store(true, Ordering::Relaxed);

    let _transition_guard = lock_account_transition()?;
    record_account_health(&account_id, observation)
        .map_err(|storage_error| storage_error.to_string())?;

    let store = load_accounts().map_err(|storage_error| storage_error.to_string())?;
    let active_id = store.active_account_id.as_deref();
    let account = store
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .ok_or_else(|| format!("Account not found: {account_id}"))?;
    Ok(AccountInfo::from_stored(account, active_id))
}

fn persist_oauth_deactivation_if_present(account_id: &str, error: &str) {
    let observation = classify_error_message(AccountHealthSource::OAuth, error);
    if !matches!(
        observation.status,
        AccountHealthStatus::AccountDeactivated | AccountHealthStatus::WorkspaceDeactivated
    ) {
        return;
    }
    let result = (|| {
        let _transition_guard = lock_account_transition()?;
        record_account_health(account_id, observation)
            .map_err(|storage_error| storage_error.to_string())?;
        Ok::<(), String>(())
    })();
    if let Err(storage_error) = result {
        eprintln!(
            "[Health] Failed to record OAuth deactivation for account {account_id}: {storage_error}"
        );
    }
}

fn replace_reauthorized_account(
    target_account_id: &str,
    candidate: StoredAccount,
) -> Result<AccountInfo, String> {
    let mut store = load_accounts().map_err(|error| error.to_string())?;
    let target_index = store
        .accounts
        .iter()
        .position(|account| account.id == target_account_id)
        .ok_or_else(|| format!("Account not found: {target_account_id}"))?;

    ensure_same_chatgpt_identity(&store.accounts[target_index], &candidate)?;
    let previous_refresh_token = match &store.accounts[target_index].auth_data {
        AuthData::ChatGPT { refresh_token, .. } => Some(refresh_token.clone()),
        AuthData::ApiKey { .. } => None,
    };
    let next_refresh_token = match &candidate.auth_data {
        AuthData::ChatGPT { refresh_token, .. } => Some(refresh_token.clone()),
        AuthData::ApiKey { .. } => None,
    };

    {
        let target = &mut store.accounts[target_index];
        target.auth_data = candidate.auth_data;
        if candidate.email.is_some() {
            target.email = candidate.email;
        }
        if candidate.plan_type.is_some() {
            target.plan_type = candidate.plan_type;
        }
        if candidate.subscription_expires_at.is_some() {
            target.subscription_expires_at = candidate.subscription_expires_at;
        }
    }

    let is_active = store.active_account_id.as_deref() == Some(target_account_id);
    let active_home = if is_active {
        Some(
            store
                .active_account_home
                .clone()
                .unwrap_or(get_codex_home_identity().map_err(|error| error.to_string())?),
        )
    } else {
        None
    };
    if let Some(active_home) = active_home.as_deref() {
        store.active_account_home = Some(active_home.to_string());
        store.pending_auth_sync_account_id = Some(target_account_id.to_string());
        store.pending_auth_sync_home = Some(active_home.to_string());
    }
    if let (Some(previous), Some(next)) = (
        previous_refresh_token.as_deref(),
        next_refresh_token.as_deref(),
    ) {
        if previous != next {
            remember_consumed_refresh_token(&mut store, previous);
        }
    }

    let updated = store.accounts[target_index].clone();
    save_accounts(&store).map_err(|error| error.to_string())?;

    if let Some(active_home) = active_home.as_deref() {
        sync_account_auth_file_at_home(&updated, Path::new(active_home)).map_err(|error| {
            format!(
                "New authorization was saved, but the active auth.json could not be updated: {error:#}"
            )
        })?;
        finalize_account_auth_sync(target_account_id, active_home).map_err(|error| {
            format!(
                "New authorization was saved, but its auth.json transition could not be finalized: {error:#}"
            )
        })?;
    }

    let latest = load_accounts().map_err(|error| error.to_string())?;
    let active_id = latest.active_account_id.as_deref();
    let account = latest
        .accounts
        .iter()
        .find(|account| account.id == target_account_id)
        .ok_or_else(|| "Reauthorized account disappeared after saving".to_string())?;
    Ok(AccountInfo::from_stored(account, active_id))
}

fn ensure_same_chatgpt_identity(
    existing: &StoredAccount,
    candidate: &StoredAccount,
) -> Result<(), String> {
    let (existing_id, existing_email) = chatgpt_identity(existing)?;
    let (candidate_id, candidate_email) = chatgpt_identity(candidate)?;

    if let (Some(existing_id), Some(candidate_id)) =
        (existing_id.as_deref(), candidate_id.as_deref())
    {
        if existing_id == candidate_id {
            return Ok(());
        }
        return Err(
            "The signed-in ChatGPT account does not match the account being reauthorized"
                .to_string(),
        );
    }

    if existing_email.as_deref().is_some_and(|existing_email| {
        candidate_email
            .as_deref()
            .is_some_and(|candidate_email| existing_email.eq_ignore_ascii_case(candidate_email))
    }) {
        return Ok(());
    }

    Err(
        "Codex Switcher could not verify that the signed-in ChatGPT account matches the selected account"
            .to_string(),
    )
}

fn chatgpt_identity(account: &StoredAccount) -> Result<(Option<String>, Option<String>), String> {
    let AuthData::ChatGPT {
        id_token,
        account_id,
        ..
    } = &account.auth_data
    else {
        return Err("Only ChatGPT accounts can be reauthorized".to_string());
    };
    let claims = parse_chatgpt_id_token_claims(id_token);
    Ok((
        account_id.clone().or(claims.account_id),
        account.email.clone().or(claims.email),
    ))
}

/// Cancel a pending OAuth login
#[tauri::command]
pub async fn cancel_login() -> Result<(), String> {
    // Also invalidates a start_login call that has not registered its receiver
    // in PENDING_OAUTH yet.
    OAUTH_GENERATION.fetch_add(1, Ordering::SeqCst);
    let mut pending = PENDING_OAUTH.lock().unwrap();
    if let Some(pending_oauth) = pending.take() {
        pending_oauth.cancelled.store(true, Ordering::Relaxed);
    }
    Ok(())
}

fn clear_pending_oauth_if_current(cancelled: &Arc<AtomicBool>) {
    let mut pending = PENDING_OAUTH.lock().unwrap();
    let is_current = pending
        .as_ref()
        .map(|current| Arc::ptr_eq(&current.cancelled, cancelled))
        .unwrap_or(false);
    if is_current {
        pending.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_oauth_page_error_preserves_manual_source() {
        let observation = classify_error_message(
            AccountHealthSource::OAuthUserReport,
            "Authentication error\nError code: account_deactivated\nRequest ID: test-request",
        );

        assert_eq!(observation.status, AccountHealthStatus::AccountDeactivated);
        assert_eq!(observation.source, AccountHealthSource::OAuthUserReport);
        assert_eq!(
            observation.error_code.as_deref(),
            Some("account_deactivated")
        );
    }

    #[test]
    fn generic_oauth_page_error_is_not_treated_as_deactivation() {
        let observation = classify_error_message(
            AccountHealthSource::OAuthUserReport,
            "Authentication failed. Please try again.",
        );

        assert_eq!(observation.status, AccountHealthStatus::Unknown);
    }
}
