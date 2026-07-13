//! OAuth login Tauri commands

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

use crate::auth::oauth_server::{start_oauth_login, wait_for_oauth_login, OAuthLoginResult};
use crate::auth::{add_account, load_accounts, lock_credential_exchange_async, remove_account};
use crate::commands::account::{lock_account_transition, switch_account_by_id_unlocked};
use crate::types::{AccountInfo, OAuthLoginInfo};

struct PendingOAuth {
    rx: Option<oneshot::Receiver<anyhow::Result<OAuthLoginResult>>>,
    cancelled: Arc<AtomicBool>,
}

// Global state for pending OAuth login
static PENDING_OAUTH: Mutex<Option<PendingOAuth>> = Mutex::new(None);
static OAUTH_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Start the OAuth login flow
#[tauri::command]
pub async fn start_login(account_name: String) -> Result<OAuthLoginInfo, String> {
    let generation = OAUTH_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

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
    let (rx, cancelled) = {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        let pending = pending
            .as_mut()
            .ok_or_else(|| "No pending OAuth login".to_string())?;
        let rx = pending
            .rx
            .take()
            .ok_or_else(|| "OAuth login completion is already being awaited".to_string())?;
        (rx, Arc::clone(&pending.cancelled))
    };

    let account = match wait_for_oauth_login(rx).await {
        Ok(account) => account,
        Err(error) => {
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
        // Add and activate under the same transition lock so no concurrent import
        // or token refresh can overwrite the new account or backup state.
        let _transition_guard = lock_account_transition()?;
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
