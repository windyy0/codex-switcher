//! ChatGPT OAuth token refresh helpers

use anyhow::{Context, Result};
use base64::Engine;
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, LazyLock, Mutex},
};
use tokio::sync::Mutex as AsyncMutex;

use super::{
    finalize_account_auth_sync, get_codex_home_identity, load_accounts,
    lock_credential_exchange_async, remember_consumed_refresh_token, save_accounts,
    sync_account_auth_file_at_home,
};
use crate::commands::account::lock_account_transition;
use crate::types::{parse_chatgpt_id_token_claims, AuthData, StoredAccount};

const DEFAULT_ISSUER: &str = "https://auth.openai.com";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const EXPIRY_SKEW_SECONDS: i64 = 60;

/// Serialize the complete refresh exchange per account. The transition lock
/// below protects local state commits, but it must not be held across HTTP
/// awaits; this async lock prevents concurrent requests from consuming the
/// same rotating refresh token at the authorization server.
static ACCOUNT_REFRESH_LOCKS: LazyLock<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static IMPORT_REFRESH_SLOTS: LazyLock<
    Mutex<HashMap<[u8; 32], Arc<AsyncMutex<Option<RefreshTokenResponse>>>>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone, serde::Deserialize)]
struct RefreshTokenResponse {
    #[serde(default)]
    id_token: Option<String>,
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

/// Ensure the account has a non-expired ChatGPT access token.
/// Returns an updated account when a refresh was performed.
pub async fn ensure_chatgpt_tokens_fresh(account: &StoredAccount) -> Result<StoredAccount> {
    match &account.auth_data {
        AuthData::ApiKey { .. } => Ok(account.clone()),
        AuthData::ChatGPT { access_token, .. } => {
            if token_expired_or_near_expiry(access_token) {
                println!(
                    "[Auth] Access token expired/near expiry for account {}, refreshing",
                    account.name
                );
                refresh_chatgpt_tokens(account).await
            } else {
                Ok(account.clone())
            }
        }
    }
}

/// Force-refresh ChatGPT OAuth tokens for an account.
pub async fn refresh_chatgpt_tokens(account: &StoredAccount) -> Result<StoredAccount> {
    let (requested_access_token, requested_refresh_token) = match &account.auth_data {
        AuthData::ApiKey { .. } => return Ok(account.clone()),
        AuthData::ChatGPT {
            access_token,
            refresh_token,
            ..
        } => (access_token.clone(), refresh_token.clone()),
    };

    let refresh_lock = account_refresh_lock(&account.id);
    let _refresh_guard = refresh_lock.lock().await;

    // Keep exports and other processes outside the server-rotation -> durable
    // local commit window, otherwise they could snapshot or consume an
    // invalidated token. The durable account must be reloaded after this lock:
    // another process may have rotated it while this process was waiting.
    let _credential_exchange = lock_credential_exchange_async().await?;
    let before_request = {
        let _transition_guard = lock_account_transition().map_err(anyhow::Error::msg)?;
        load_latest_account(&account.id, "after waiting for the credential lock")?
    };
    let (current_access_token, current_refresh_token) = account_tokens(&before_request)?;
    if current_access_token != requested_access_token
        || current_refresh_token != requested_refresh_token
    {
        return Ok(before_request);
    }
    if current_refresh_token.is_empty() {
        anyhow::bail!("Missing refresh token for account {}", before_request.name);
    }

    let refreshed = refresh_tokens_with_refresh_token(&current_refresh_token).await?;

    // Serialize the store update and active-file sync with manual account
    // transitions. This synchronous lock is intentionally acquired only after
    // the HTTP await, while the per-account async lock remains the singleflight
    // owner for the whole refresh operation.
    let _transition_guard = lock_account_transition().map_err(anyhow::Error::msg)?;
    let store = load_accounts()?;
    let latest = store
        .accounts
        .iter()
        .find(|candidate| candidate.id == account.id)
        .cloned()
        .context("Account not found after token refresh")?;
    let (latest_access_token, latest_refresh_token) = account_tokens(&latest)?;

    // A non-refresh store writer could still replace credentials while the
    // network request is in flight. Never overwrite that newer state.
    if latest_refresh_token != current_refresh_token || latest_access_token != current_access_token
    {
        return Ok(latest);
    }

    let updated = apply_refresh_response(latest.clone(), refreshed)?;
    let is_active = store.active_account_id.as_deref() == Some(account.id.as_str());
    let active_home = if is_active {
        Some(match &store.active_account_home {
            Some(home) => home.clone(),
            None => get_codex_home_identity()?,
        })
    } else {
        None
    };

    // A rotated refresh token is canonical and may already have invalidated
    // the previous one at the server. Persist it before updating auth.json,
    // which is only a derived file and can be rebuilt on a later switch.
    persist_refreshed_account(&updated, &current_refresh_token, active_home.as_deref())
        .context("Failed to persist rotated ChatGPT credentials")?;

    if let Some(active_home) = active_home.as_deref() {
        sync_account_auth_file_at_home(&updated, Path::new(active_home)).context(
            "Refreshed credentials were saved, but active auth.json could not be synchronized",
        )?;
        finalize_account_auth_sync(&updated.id, active_home)
            .context("Failed to finish refreshed credential transition")?;
    }

    Ok(updated)
}

fn account_refresh_lock(account_id: &str) -> Arc<AsyncMutex<()>> {
    let mut locks = ACCOUNT_REFRESH_LOCKS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    locks
        .entry(account_id.to_string())
        .or_insert_with(|| Arc::new(AsyncMutex::new(())))
        .clone()
}

fn load_latest_account(account_id: &str, phase: &str) -> Result<StoredAccount> {
    load_accounts()?
        .accounts
        .into_iter()
        .find(|candidate| candidate.id == account_id)
        .with_context(|| format!("Account not found {phase}"))
}

fn account_tokens(account: &StoredAccount) -> Result<(String, String)> {
    match &account.auth_data {
        AuthData::ApiKey { .. } => anyhow::bail!("Account is no longer using ChatGPT OAuth"),
        AuthData::ChatGPT {
            access_token,
            refresh_token,
            ..
        } => Ok((access_token.clone(), refresh_token.clone())),
    }
}

fn apply_refresh_response(
    mut account: StoredAccount,
    refreshed: RefreshTokenResponse,
) -> Result<StoredAccount> {
    let (stored_id_token, stored_account_id) = match &account.auth_data {
        AuthData::ApiKey { .. } => anyhow::bail!("Account is not using ChatGPT OAuth"),
        AuthData::ChatGPT {
            id_token,
            account_id,
            ..
        } => (id_token.clone(), account_id.clone()),
    };

    let next_id_token = refreshed.id_token.unwrap_or(stored_id_token);
    let claims = parse_chatgpt_id_token_claims(&next_id_token);
    let next_account_id = claims.account_id.clone().or(stored_account_id);

    let AuthData::ChatGPT {
        id_token,
        access_token,
        refresh_token,
        account_id,
    } = &mut account.auth_data
    else {
        unreachable!("account auth mode was checked above")
    };
    *id_token = next_id_token;
    *access_token = refreshed.access_token;
    if let Some(next_refresh_token) = refreshed.refresh_token {
        *refresh_token = next_refresh_token;
    }
    *account_id = next_account_id;

    if let Some(email) = claims.email {
        account.email = Some(email);
    }
    if let Some(plan_type) = claims.plan_type {
        account.plan_type = Some(plan_type);
    }
    if let Some(subscription_expires_at) = claims.subscription_expires_at {
        account.subscription_expires_at = Some(subscription_expires_at);
    }

    Ok(account)
}

fn persist_refreshed_account(
    updated: &StoredAccount,
    consumed_refresh_token: &str,
    active_home: Option<&str>,
) -> Result<()> {
    // switch_to_account may update config backup flags, so always reload the
    // store immediately before replacing the account record.
    let mut store = load_accounts()?;
    let account = store
        .accounts
        .iter_mut()
        .find(|candidate| candidate.id == updated.id)
        .context("Account not found while saving refreshed tokens")?;
    *account = updated.clone();
    if store.active_account_id.as_deref() == Some(updated.id.as_str()) {
        let active_home = active_home.context("Active account has no CODEX_HOME identity")?;
        store.active_account_home = Some(active_home.to_string());
        store.pending_auth_sync_account_id = Some(updated.id.clone());
        store.pending_auth_sync_home = Some(active_home.to_string());
    }
    let (_, persisted_refresh_token) = account_tokens(updated)?;
    if persisted_refresh_token != consumed_refresh_token {
        remember_consumed_refresh_token(&mut store, consumed_refresh_token);
    }
    save_accounts(&store)
}

/// Build a new ChatGPT account from a refresh token.
/// This is used by slim import to recreate full credentials.
pub async fn create_chatgpt_account_from_refresh_token(
    account_name: String,
    refresh_token: String,
) -> Result<StoredAccount> {
    if refresh_token.trim().is_empty() {
        anyhow::bail!("Missing refresh token for account {account_name}");
    }

    // Reuse one successful rotation for concurrent imports of the same source
    // token. The map key is a digest so the original credential is not retained.
    let token_hash: [u8; 32] = Sha256::digest(refresh_token.as_bytes()).into();
    let slot = {
        let mut slots = IMPORT_REFRESH_SLOTS
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        slots
            .entry(token_hash)
            .or_insert_with(|| Arc::new(AsyncMutex::new(None)))
            .clone()
    };
    let mut cached = slot.lock().await;
    let refreshed_result = if let Some(refreshed) = cached.as_ref() {
        Ok(refreshed.clone())
    } else {
        match refresh_tokens_with_refresh_token(&refresh_token).await {
            Ok(refreshed) => {
                *cached = Some(refreshed.clone());
                Ok(refreshed)
            }
            Err(error) => Err(error),
        }
    };
    drop(cached);

    // Callers that already joined this operation retain the Arc and can reuse
    // its result; removing the map entry prevents later, independent imports
    // from retaining credentials or reusing a rotated response.
    {
        let mut slots = IMPORT_REFRESH_SLOTS
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if slots
            .get(&token_hash)
            .is_some_and(|current| Arc::ptr_eq(current, &slot))
        {
            slots.remove(&token_hash);
        }
    }
    let refreshed = refreshed_result?;
    let id_token = refreshed
        .id_token
        .context("Refresh response did not include id_token")?;
    let next_refresh_token = refreshed.refresh_token.unwrap_or(refresh_token);
    let claims = parse_chatgpt_id_token_claims(&id_token);

    Ok(StoredAccount::new_chatgpt(
        account_name,
        claims.email,
        claims.plan_type,
        claims.subscription_expires_at,
        id_token,
        refreshed.access_token,
        next_refresh_token,
        claims.account_id,
    ))
}

fn token_expired_or_near_expiry(access_token: &str) -> bool {
    match parse_jwt_exp(access_token) {
        Some(expiry) => expiry <= Utc::now().timestamp() + EXPIRY_SKEW_SECONDS,
        None => false,
    }
}

fn parse_jwt_exp(token: &str) -> Option<i64> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }

    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    json.get("exp").and_then(|v| v.as_i64())
}

async fn refresh_tokens_with_refresh_token(refresh_token: &str) -> Result<RefreshTokenResponse> {
    let client = reqwest::Client::new();
    let body = format!(
        "grant_type=refresh_token&refresh_token={}&client_id={}",
        urlencoding::encode(refresh_token),
        urlencoding::encode(CLIENT_ID),
    );

    // Refresh-token exchange is not safely retryable: a transport error may
    // arrive after the authorization server consumed the rotating token. Never
    // replay the same token automatically.
    let response = client
        .post(format!("{DEFAULT_ISSUER}/oauth/token"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .context("Failed to send token refresh request; the request was not retried")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Token refresh failed: {status} - {body}");
    }

    response
        .json::<RefreshTokenResponse>()
        .await
        .context("Failed to parse token refresh response")
}

#[cfg(test)]
mod tests {
    use super::{account_refresh_lock, apply_refresh_response, RefreshTokenResponse};
    use crate::types::{AuthData, StoredAccount};
    use std::sync::Arc;

    #[test]
    fn refresh_locks_are_shared_per_account_only() {
        let first = account_refresh_lock("account-a");
        let same = account_refresh_lock("account-a");
        let other = account_refresh_lock("account-b");

        assert!(Arc::ptr_eq(&first, &same));
        assert!(!Arc::ptr_eq(&first, &other));
    }

    #[test]
    fn refresh_response_preserves_refresh_token_when_server_omits_rotation() {
        let account = StoredAccount::new_chatgpt(
            "ChatGPT".into(),
            None,
            None,
            None,
            "old-id".into(),
            "old-access".into(),
            "old-refresh".into(),
            Some("old-account".into()),
        );
        let updated = apply_refresh_response(
            account,
            RefreshTokenResponse {
                id_token: None,
                access_token: "new-access".into(),
                refresh_token: None,
            },
        )
        .expect("refresh response should apply");

        let AuthData::ChatGPT {
            id_token,
            access_token,
            refresh_token,
            account_id,
        } = updated.auth_data
        else {
            panic!("expected ChatGPT account")
        };
        assert_eq!(id_token, "old-id");
        assert_eq!(access_token, "new-access");
        assert_eq!(refresh_token, "old-refresh");
        assert_eq!(account_id.as_deref(), Some("old-account"));
    }
}
