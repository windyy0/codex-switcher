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
    finalize_account_auth_sync, get_codex_home_identity, has_consumed_refresh_token, load_accounts,
    lock_credential_exchange_async, read_current_auth, remember_consumed_refresh_token,
    save_accounts, sync_account_auth_file_at_home,
};
use crate::commands::account::lock_account_transition;
use crate::types::{
    parse_chatgpt_id_token_claims, AccountsStore, AuthData, AuthDotJson, StoredAccount, TokenData,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthFileTokenMerge {
    Unchanged,
    Updated,
    IdentityMismatch,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RefreshTokenResponse {
    #[serde(default)]
    id_token: Option<String>,
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

/// Import credentials that the active Codex client rotated in auth.json.
///
/// The account store is canonical for inactive accounts, but auth.json is a
/// live credential cache for the active account and Codex may refresh it while
/// Switcher is not involved. Reconcile that one account before authenticated
/// requests so Switcher never consumes the stale pre-rotation token again.
pub async fn sync_active_account_from_codex_auth(
    account_id: &str,
) -> Result<Option<StoredAccount>> {
    let _credential_exchange = lock_credential_exchange_async().await?;
    let _transition_guard = lock_account_transition().map_err(anyhow::Error::msg)?;
    sync_active_account_from_codex_auth_unlocked(account_id)
}

/// Caller holds the credential-exchange and account-transition locks, in that order.
pub(crate) fn sync_active_account_from_codex_auth_unlocked(
    account_id: &str,
) -> Result<Option<StoredAccount>> {
    Ok(sync_current_codex_auth_unlocked()?.filter(|account| account.id == account_id))
}

/// The live file, not the UI's active marker, identifies the outgoing account.
/// Do not change that marker here: it also describes the managed config overlay.
pub(crate) fn sync_current_codex_auth_unlocked() -> Result<Option<StoredAccount>> {
    let mut store = load_accounts()?;
    let current_home = get_codex_home_identity()?;
    let Some(auth) = read_current_auth()? else {
        return Ok(None);
    };
    // A recorded account in another home must be reconciled in that home.
    if store
        .active_account_home
        .as_deref()
        .is_some_and(|home| home != current_home)
        && store.accounts.iter().any(|account| {
            store.active_account_id.as_deref() == Some(&account.id)
                && auth_matches_account(&auth, account)
        })
    {
        return Ok(None);
    }
    let updated = reconcile_live_auth(&mut store, &auth);
    if updated.is_some() {
        save_accounts(&store)?;
    }
    Ok(updated)
}

/// Shared by normal reconciliation and crash recovery. Ambiguous/missing IDs
/// must never select an account, even if the email or refresh token matches.
pub(crate) fn reconcile_live_auth(
    store: &mut AccountsStore,
    auth: &AuthDotJson,
) -> Option<StoredAccount> {
    let tokens = auth.tokens.as_ref()?;
    let mut matches = store
        .accounts
        .iter()
        .enumerate()
        .filter(|(_, account)| auth_matches_account(auth, account));
    let index = matches.next()?.0;
    if matches.next().is_some() {
        return None;
    }
    let mut updated = store.accounts[index].clone();
    if !merge_live_account_tokens(store, &mut updated, tokens) {
        return None;
    }
    if updated.auth_data == store.accounts[index].auth_data {
        return None;
    }
    store.accounts[index] = updated.clone();
    Some(updated)
}

pub(crate) fn merge_live_account_tokens(
    store: &mut AccountsStore,
    account: &mut StoredAccount,
    tokens: &TokenData,
) -> bool {
    if has_consumed_refresh_token(store, &tokens.refresh_token) {
        return false;
    }
    let previous = match &account.auth_data {
        AuthData::ChatGPT { refresh_token, .. } => refresh_token.clone(),
        AuthData::ApiKey { .. } => return false,
    };
    match merge_auth_file_tokens(account, tokens.clone()) {
        AuthFileTokenMerge::IdentityMismatch => false,
        AuthFileTokenMerge::Unchanged => true,
        AuthFileTokenMerge::Updated => {
            if previous != tokens.refresh_token && !previous.is_empty() {
                remember_consumed_refresh_token(store, &previous);
            }
            true
        }
    }
}

pub(crate) fn auth_matches_account(auth: &AuthDotJson, account: &StoredAccount) -> bool {
    match &account.auth_data {
        AuthData::ApiKey { key } => {
            auth.tokens.is_none() && auth.openai_api_key.as_deref() == Some(key)
        }
        AuthData::ChatGPT { .. } => {
            auth.openai_api_key.is_none()
                && auth.tokens.as_ref().is_some_and(|tokens| {
                    merge_auth_file_tokens(&mut account.clone(), tokens.clone())
                        != AuthFileTokenMerge::IdentityMismatch
                })
        }
    }
}

/// Only publish to a file that still belongs to this account. A stale active
/// marker must not overwrite an external login to a different account.
pub(crate) fn account_auth_sync_home(
    store: &AccountsStore,
    account: &StoredAccount,
) -> Result<Option<String>> {
    let home = get_codex_home_identity()?;
    let marked_here = store.active_account_id.as_deref() == Some(&account.id)
        && store
            .active_account_home
            .as_deref()
            .is_none_or(|active_home| active_home == home);
    Ok(match read_current_auth()? {
        Some(auth) if auth_matches_account(&auth, account) => Some(home),
        None if marked_here => Some(home),
        _ => None,
    })
}

/// Raw-token equality is only an ownership guard, never proof of account ID.
pub(crate) fn ensure_import_token_not_in_use(refresh_token: &str) -> Result<()> {
    let store = load_accounts()?;
    let mut homes = vec![get_codex_home_identity()?];
    if let Some(home) = store
        .active_account_home
        .filter(|home| !homes.contains(home))
    {
        homes.push(home);
    }
    for home in homes {
        if super::read_auth_at_home(Path::new(&home))?
            .and_then(|auth| auth.tokens)
            .is_some_and(|tokens| tokens.refresh_token == refresh_token)
            && crate::commands::process::codex_is_running().map_err(anyhow::Error::msg)?
        {
            anyhow::bail!(
                "Cannot import credentials used by a running Codex session; close Codex first"
            );
        }
    }
    Ok(())
}

fn merge_auth_file_tokens(account: &mut StoredAccount, tokens: TokenData) -> AuthFileTokenMerge {
    let AuthData::ChatGPT {
        id_token: stored_id_token,
        access_token: stored_access_token,
        refresh_token: stored_refresh_token,
        account_id: stored_account_id,
    } = &account.auth_data
    else {
        return AuthFileTokenMerge::IdentityMismatch;
    };

    let stored_claims = parse_chatgpt_id_token_claims(stored_id_token);
    let incoming_claims = parse_chatgpt_id_token_claims(&tokens.id_token);
    let stored_identity = consistent_account_identity(
        stored_claims.account_id.as_deref(),
        stored_account_id.as_deref(),
    );
    let incoming_identity = consistent_account_identity(
        incoming_claims.account_id.as_deref(),
        tokens.account_id.as_deref(),
    );
    let same_identity = match (stored_identity, incoming_identity) {
        (Some(stored), Some(incoming)) => stored == incoming,
        _ => false,
    };
    if !same_identity {
        return AuthFileTokenMerge::IdentityMismatch;
    }

    let next_account_id = incoming_identity.map(str::to_owned);
    if stored_id_token == &tokens.id_token
        && stored_access_token == &tokens.access_token
        && stored_refresh_token == &tokens.refresh_token
        && stored_account_id == &next_account_id
    {
        return AuthFileTokenMerge::Unchanged;
    }

    account.auth_data = AuthData::ChatGPT {
        id_token: tokens.id_token,
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        account_id: next_account_id,
    };
    if let Some(email) = incoming_claims.email {
        account.email = Some(email);
    }
    if let Some(plan_type) = incoming_claims.plan_type {
        account.plan_type = Some(plan_type);
    }
    // auth.json is authoritative for credentials, not live subscription
    // metadata. Its ID token can keep the pre-renewal entitlement after Codex
    // rotates an access token, so only use this claim as a missing-value
    // fallback. The accounts-check endpoint owns later subscription updates.
    if account.subscription_expires_at.is_none() {
        account.subscription_expires_at = incoming_claims.subscription_expires_at;
    }
    AuthFileTokenMerge::Updated
}

/// Email alone cannot distinguish personal and workspace accounts. Reject
/// missing identities as well as disagreement between a token and its envelope.
fn consistent_account_identity<'a>(
    claim: Option<&'a str>,
    field: Option<&'a str>,
) -> Option<&'a str> {
    let claim = claim.filter(|value| !value.trim().is_empty());
    let field = field.filter(|value| !value.trim().is_empty());
    match (claim, field) {
        (Some(claim), Some(field)) if claim != field => None,
        (Some(value), _) | (_, Some(value)) => Some(value),
        _ => None,
    }
}

/// Reconcile live credentials and check both OAuth tokens. A running Codex
/// client owns active-account refreshes; requests may still use its live token.
pub async fn ensure_chatgpt_tokens_fresh(account: &StoredAccount) -> Result<StoredAccount> {
    refresh_chatgpt_tokens_guarded(account, false).await
}

/// Force-refresh ChatGPT OAuth tokens for an account.
pub async fn refresh_chatgpt_tokens(account: &StoredAccount) -> Result<StoredAccount> {
    refresh_chatgpt_tokens_guarded(account, true).await
}

async fn refresh_chatgpt_tokens_guarded(
    account: &StoredAccount,
    force: bool,
) -> Result<StoredAccount> {
    if matches!(account.auth_data, AuthData::ApiKey { .. }) {
        return Ok(account.clone());
    }
    let refresh_lock = account_refresh_lock(&account.id);
    let _refresh_guard = refresh_lock.lock().await;

    // Keep exports and other processes outside the server-rotation -> durable
    // local commit window, otherwise they could snapshot or consume an
    // invalidated token. The durable account must be reloaded after this lock:
    // another process may have rotated it while this process was waiting.
    let _credential_exchange = lock_credential_exchange_async().await?;
    refresh_chatgpt_tokens_with_credential_lock(account, force).await
}

/// Switching already owns the global credential lock. Do not acquire a
/// per-account refresh lock here: another refresher may hold it while waiting
/// for that global lock. Never hold the synchronous transition lock over HTTP.
pub(crate) async fn ensure_chatgpt_tokens_fresh_with_credential_lock(
    account: &StoredAccount,
) -> Result<StoredAccount> {
    if matches!(account.auth_data, AuthData::ApiKey { .. }) {
        return Ok(account.clone());
    }
    refresh_chatgpt_tokens_with_credential_lock(account, false).await
}

async fn refresh_chatgpt_tokens_with_credential_lock(
    account: &StoredAccount,
    force: bool,
) -> Result<StoredAccount> {
    let (requested_access_token, requested_refresh_token) = account_tokens(account)?;
    let (before_request, is_active) = {
        let _transition_guard = lock_account_transition().map_err(anyhow::Error::msg)?;
        sync_current_codex_auth_unlocked()?;
        let store = load_accounts()?;
        let latest = load_latest_account(&account.id, "after waiting for the credential lock")?;
        let (_, refresh_token) = account_tokens(&latest)?;
        let is_active = store.active_account_id.as_deref() == Some(&account.id)
            || read_current_auth()?.is_some_and(|auth| {
                auth_matches_account(&auth, &latest)
                    // A conflicting/missing ID forbids merging, but sharing the
                    // live refresh token must still prevent an independent use.
                    || auth.tokens.as_ref().is_some_and(|tokens| {
                        !refresh_token.is_empty() && tokens.refresh_token == refresh_token
                    })
            });
        let current_home = get_codex_home_identity()?;
        if store.active_account_id.as_deref() == Some(&account.id)
            && store
                .active_account_home
                .as_deref()
                .is_some_and(|home| home != current_home)
        {
            anyhow::bail!("The active account belongs to a different CODEX_HOME; use that home before refreshing");
        }
        (latest, is_active)
    };
    let (current_access_token, current_refresh_token) = account_tokens(&before_request)?;
    let credentials_changed = current_access_token != requested_access_token
        || current_refresh_token != requested_refresh_token;
    if (!force || credentials_changed) && !chatgpt_tokens_need_refresh(&before_request) {
        return Ok(before_request);
    }
    if is_active && crate::commands::process::codex_is_running().map_err(anyhow::Error::msg)? {
        if force && !credentials_changed {
            anyhow::bail!("Active account refresh is managed by the running Codex app; retry after Codex updates its credentials or close Codex first");
        }
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

    let (updated, token_error) = apply_refresh_response(latest.clone(), refreshed)?;
    // An unusable ID token must not be published or scheduled for recovery.
    // Also avoid overwriting live auth if Codex started during the HTTP request.
    let active_home = if token_error.is_none()
        && matches!(crate::commands::process::codex_is_running(), Ok(false))
    {
        // Live-file inspection can fail after the server has already rotated
        // credentials. Always retain the response, but do not publish blindly.
        account_auth_sync_home(&store, &updated).unwrap_or(None)
    } else {
        None
    };

    // A rotated refresh token is canonical and may already have invalidated
    // the previous one at the server. Persist it before updating auth.json,
    // which is only a derived file and can be rebuilt on a later switch.
    persist_refreshed_account(&updated, &current_refresh_token, active_home.as_deref())
        .context("Failed to persist rotated ChatGPT credentials")?;

    if let Some(error) = token_error {
        return Err(error);
    }

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
) -> Result<(StoredAccount, Option<anyhow::Error>)> {
    let (stored_id_token, stored_account_id) = match &account.auth_data {
        AuthData::ApiKey { .. } => anyhow::bail!("Account is not using ChatGPT OAuth"),
        AuthData::ChatGPT {
            id_token,
            account_id,
            ..
        } => (id_token.clone(), account_id.clone()),
    };

    let candidate = refreshed.id_token.as_deref().unwrap_or(&stored_id_token);
    let token_error = id_token_needs_refresh_at(candidate, Utc::now().timestamp()).then(|| {
        anyhow::anyhow!(
            "Token refresh did not return a usable fresh id_token; sign in to the account again"
        )
    });
    let next_id_token = if token_error.is_some() {
        stored_id_token
    } else {
        refreshed.id_token.unwrap_or(stored_id_token)
    };
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
    // A refresh response can omit a newly issued ID token and retain stale
    // entitlement claims. Preserve metadata previously fetched from the live
    // accounts-check endpoint, using the ID-token claim only as a fallback.
    if account.subscription_expires_at.is_none() {
        account.subscription_expires_at = claims.subscription_expires_at;
    }

    Ok((account, token_error))
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
    if let Some(active_home) = active_home {
        if store.active_account_id.as_deref() == Some(updated.id.as_str()) {
            store.active_account_home = Some(active_home.to_string());
        }
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
    ensure_import_token_not_in_use(&refresh_token)?;

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

pub(crate) fn chatgpt_tokens_need_refresh(account: &StoredAccount) -> bool {
    match &account.auth_data {
        AuthData::ApiKey { .. } => false,
        AuthData::ChatGPT {
            id_token,
            access_token,
            ..
        } => {
            let now = Utc::now().timestamp();
            id_token_needs_refresh_at(id_token, now)
                || token_expired_or_near_expiry_at(access_token, now)
        }
    }
}

fn id_token_needs_refresh_at(token: &str, now: i64) -> bool {
    parse_jwt_exp(token).is_none_or(|expiry| expiry <= now + EXPIRY_SKEW_SECONDS)
}

fn token_expired_or_near_expiry_at(access_token: &str, now: i64) -> bool {
    match parse_jwt_exp(access_token) {
        Some(expiry) => expiry <= now + EXPIRY_SKEW_SECONDS,
        None => access_token.trim().is_empty(),
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
    #[cfg(test)]
    if super::test_support::is_active() {
        return serde_json::from_value(super::test_support::refresh_response(refresh_token).await?)
            .context("Invalid mock refresh response");
    }
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
        .timeout(std::time::Duration::from_secs(30))
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
    use super::{
        account_refresh_lock, apply_refresh_response, chatgpt_tokens_need_refresh,
        ensure_chatgpt_tokens_fresh, id_token_needs_refresh_at, merge_auth_file_tokens,
        refresh_chatgpt_tokens, sync_active_account_from_codex_auth,
        token_expired_or_near_expiry_at, AuthFileTokenMerge, RefreshTokenResponse,
    };
    use crate::auth::test_support::{account, jwt, seed_accounts, AuthTestEnv};
    use crate::auth::{
        get_account, get_codex_auth_file, has_consumed_refresh_token, load_accounts,
        read_current_auth, recover_pending_account_transition, sync_account_auth_file,
    };
    use crate::types::{AuthData, StoredAccount, TokenData};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use std::sync::Arc;

    fn id_token(account_id: &str, email: &str, plan_type: &str) -> String {
        id_token_with_subscription(account_id, email, plan_type, None)
    }

    fn id_token_with_subscription(
        account_id: &str,
        email: &str,
        plan_type: &str,
        subscription_expires_at: Option<&str>,
    ) -> String {
        let payload = serde_json::json!({
            "exp": chrono::Utc::now().timestamp() + 3600,
            "email": email,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id,
                "chatgpt_plan_type": plan_type,
                "chatgpt_subscription_active_until": subscription_expires_at,
            }
        });
        format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).expect("payload should serialize"))
        )
    }

    #[test]
    fn refresh_locks_are_shared_per_account_only() {
        let first = account_refresh_lock("account-a");
        let same = account_refresh_lock("account-a");
        let other = account_refresh_lock("account-b");

        assert!(Arc::ptr_eq(&first, &same));
        assert!(!Arc::ptr_eq(&first, &other));
    }

    #[test]
    fn expiry_checks_require_a_usable_id_token_and_keep_opaque_access_tokens() {
        let now = chrono::Utc::now().timestamp();
        for (expiry, needs_refresh) in [(now - 1, true), (now + 60, true), (now + 61, false)] {
            let token = jwt(Some("a"), Some(expiry));
            assert_eq!(id_token_needs_refresh_at(&token, now), needs_refresh);
            assert_eq!(token_expired_or_near_expiry_at(&token, now), needs_refresh);
        }
        assert!(id_token_needs_refresh_at("malformed", now));
        assert!(id_token_needs_refresh_at(&jwt(Some("a"), None), now));
        assert!(!token_expired_or_near_expiry_at("opaque-access", now));
        assert!(token_expired_or_near_expiry_at("", now));
        assert!(!chatgpt_tokens_need_refresh(&StoredAccount::new_api_key(
            "API".into(),
            "key".into()
        )));
    }

    #[test]
    fn live_auth_requires_nonempty_consistent_identities_on_both_sides() {
        let expiry = Some(chrono::Utc::now().timestamp() + 3600);
        // Claims, envelope ID for stored and incoming credentials. All share
        // the same email, including cases with byte-identical ID tokens.
        for (stored_claim, stored_field, incoming_claim, incoming_field, allowed) in [
            (Some("a"), Some("a"), Some("a"), Some("a"), true),
            (None, Some("a"), Some("a"), None, true),
            (Some("a"), None, None, Some("a"), true),
            (None, None, None, None, false),
            (None, None, Some("a"), Some("a"), false),
            (Some("a"), Some("a"), None, None, false),
            (Some("a"), Some("a"), Some("b"), Some("b"), false),
            (Some("a"), Some("b"), Some("a"), Some("a"), false),
            (Some("a"), Some("a"), Some("a"), Some("b"), false),
            (Some(""), Some(" "), Some(""), Some(" "), false),
        ] {
            let mut account = account("a");
            if let AuthData::ChatGPT {
                id_token,
                account_id,
                ..
            } = &mut account.auth_data
            {
                *id_token = jwt(stored_claim, expiry);
                *account_id = stored_field.map(str::to_owned);
            }
            let original = serde_json::to_value(&account).unwrap();
            let result = merge_auth_file_tokens(
                &mut account,
                TokenData {
                    id_token: jwt(incoming_claim, expiry),
                    access_token: "live-access".into(),
                    refresh_token: "live-refresh".into(),
                    account_id: incoming_field.map(str::to_owned),
                },
            );
            assert_eq!(result == AuthFileTokenMerge::Updated, allowed);
            if !allowed {
                assert_eq!(serde_json::to_value(&account).unwrap(), original);
            }
        }
    }

    #[tokio::test]
    async fn running_codex_owns_active_refreshes_but_not_inactive_ones() {
        let env = AuthTestEnv::new();
        let active = account("active");
        let inactive = account("inactive");
        seed_accounts(vec![active.clone(), inactive.clone()], Some(&active.id));
        let mut live = active.clone();
        if let AuthData::ChatGPT {
            refresh_token,
            access_token,
            ..
        } = &mut live.auth_data
        {
            *refresh_token = "live-refresh".into();
            *access_token = "live-access".into();
        }
        sync_account_auth_file(&live).unwrap();
        env.running(true);
        let reconciled = refresh_chatgpt_tokens(&active).await.unwrap();
        assert_eq!(
            super::account_tokens(&reconciled).unwrap(),
            ("live-access".into(), "live-refresh".into())
        );
        assert!(env.requests().is_empty());
        let error = refresh_chatgpt_tokens(&reconciled).await.unwrap_err();
        assert!(error.to_string().contains("running Codex"));
        assert_ne!(
            crate::account_health::classify_error_message(
                crate::types::AccountHealthSource::Usage,
                &error.to_string()
            )
            .status,
            crate::types::AccountHealthStatus::ReauthRequired
        );

        // Expired live access can be sent once, but must not trigger an
        // independent rotation while Codex owns the account.
        if let AuthData::ChatGPT { access_token, .. } = &mut live.auth_data {
            *access_token = jwt(None, Some(1));
        }
        sync_account_auth_file(&live).unwrap();
        let current = ensure_chatgpt_tokens_fresh(&reconciled).await.unwrap();
        assert!(chatgpt_tokens_need_refresh(&current));
        assert!(env.requests().is_empty());

        env.respond(serde_json::json!({"access_token":"inactive-access", "refresh_token":"inactive-rotated"}));
        let refreshed = refresh_chatgpt_tokens(&inactive).await.unwrap();
        assert_eq!(
            super::account_tokens(&refreshed).unwrap().1,
            "inactive-rotated"
        );
        assert_eq!(env.requests(), ["inactive-refresh"]);
        assert_eq!(
            read_current_auth()
                .unwrap()
                .unwrap()
                .tokens
                .unwrap()
                .refresh_token,
            "live-refresh"
        );
    }

    #[tokio::test]
    async fn active_refresh_fails_closed_when_process_inspection_fails() {
        let env = AuthTestEnv::new();
        let active = account("active");
        seed_accounts(vec![active.clone()], Some(&active.id));
        env.process_error();
        assert!(refresh_chatgpt_tokens(&active)
            .await
            .unwrap_err()
            .to_string()
            .contains("process inspection"));
        assert!(env.requests().is_empty());
    }

    #[tokio::test]
    async fn concurrent_refreshes_consume_a_source_token_only_once() {
        let env = AuthTestEnv::new();
        let account = account("inactive");
        seed_accounts(vec![account.clone()], None);
        env.respond(
            serde_json::json!({"access_token":"new-access", "refresh_token":"new-refresh"}),
        );
        let (first, second) = tokio::join!(
            refresh_chatgpt_tokens(&account),
            refresh_chatgpt_tokens(&account)
        );
        assert_eq!(
            super::account_tokens(&first.unwrap()).unwrap(),
            super::account_tokens(&second.unwrap()).unwrap()
        );
        assert_eq!(env.requests(), ["inactive-refresh"]);
    }

    #[tokio::test]
    async fn transport_failure_is_never_replayed_automatically() {
        let env = AuthTestEnv::new();
        let account = account("inactive");
        seed_accounts(vec![account.clone()], None);
        env.fail_refresh();
        assert!(refresh_chatgpt_tokens(&account).await.is_err());
        assert_eq!(env.requests(), ["inactive-refresh"]);
        assert_eq!(
            super::account_tokens(&get_account(&account.id).unwrap().unwrap())
                .unwrap()
                .1,
            "inactive-refresh"
        );
    }

    #[tokio::test]
    async fn missing_or_invalid_id_token_preserves_rotation_without_publishing_it() {
        for replacement in [
            None,
            Some("malformed".to_string()),
            Some(jwt(Some("active"), Some(1))),
        ] {
            let env = AuthTestEnv::new();
            let mut active = account("active");
            if let AuthData::ChatGPT { id_token, .. } = &mut active.auth_data {
                *id_token = jwt(Some("active"), Some(1));
            }
            seed_accounts(vec![active.clone()], Some(&active.id));
            let original_auth = std::fs::read(get_codex_auth_file().unwrap()).unwrap();
            env.respond(serde_json::json!({"id_token":replacement, "access_token":"new-access", "refresh_token":"rotated"}));
            assert!(ensure_chatgpt_tokens_fresh(&active)
                .await
                .unwrap_err()
                .to_string()
                .contains("id_token"));
            let store = load_accounts().unwrap();
            assert_eq!(
                super::account_tokens(&store.accounts[0]).unwrap().1,
                "rotated"
            );
            assert!(has_consumed_refresh_token(&store, "active-refresh"));
            assert!(store.pending_auth_sync_account_id.is_none());
            // Recovery and live reconciliation must not undo the rotation or
            // write the unusable response back into the running client's file.
            recover_pending_account_transition().unwrap();
            assert!(sync_active_account_from_codex_auth(&active.id)
                .await
                .unwrap()
                .is_none());
            assert_eq!(
                std::fs::read(get_codex_auth_file().unwrap()).unwrap(),
                original_auth
            );
            assert_eq!(
                super::account_tokens(&get_account(&active.id).unwrap().unwrap())
                    .unwrap()
                    .1,
                "rotated"
            );
        }
    }

    #[tokio::test]
    async fn live_sync_uses_actual_identity_but_does_not_cross_recorded_homes() {
        let _env = AuthTestEnv::new();
        let active = account("active");
        let inactive = account("inactive");
        seed_accounts(vec![active.clone(), inactive.clone()], Some(&active.id));
        let mut live = inactive.clone();
        if let AuthData::ChatGPT { refresh_token, .. } = &mut live.auth_data {
            *refresh_token = "live-inactive".into();
        }
        sync_account_auth_file(&live).unwrap();
        assert!(sync_active_account_from_codex_auth(&inactive.id)
            .await
            .unwrap()
            .is_some());
        assert!(sync_active_account_from_codex_auth(&active.id)
            .await
            .unwrap()
            .is_none());
        let mut store = load_accounts().unwrap();
        store.active_account_home = Some("another-home".into());
        crate::auth::save_accounts(&store).unwrap();
        live = active.clone();
        if let AuthData::ChatGPT { refresh_token, .. } = &mut live.auth_data {
            *refresh_token = "live-active".into();
        }
        sync_account_auth_file(&live).unwrap();
        assert!(sync_active_account_from_codex_auth(&active.id)
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            super::account_tokens(&get_account(&active.id).unwrap().unwrap())
                .unwrap()
                .1,
            "active-refresh"
        );
    }

    #[tokio::test]
    async fn missing_or_stale_active_marker_does_not_allow_live_account_refresh() {
        for has_marker in [false, true] {
            let env = AuthTestEnv::new();
            let a = account("a");
            let mut live = account("live");
            if let AuthData::ChatGPT { access_token, .. } = &mut live.auth_data {
                *access_token = jwt(Some("live"), Some(1));
            }
            seed_accounts(
                vec![a.clone(), live.clone()],
                has_marker.then_some(a.id.as_str()),
            );
            sync_account_auth_file(&live).unwrap();
            env.running(true);
            let current = ensure_chatgpt_tokens_fresh(&live).await.unwrap();
            assert!(chatgpt_tokens_need_refresh(&current));
            assert!(refresh_chatgpt_tokens(&current).await.is_err());
            assert!(env.requests().is_empty());
        }
    }

    #[tokio::test]
    async fn unmarked_live_account_refresh_syncs_without_claiming_config_ownership() {
        let env = AuthTestEnv::new();
        let active = account("live");
        seed_accounts(vec![active.clone()], None);
        sync_account_auth_file(&active).unwrap();
        env.respond(serde_json::json!({"access_token":"new-access","refresh_token":"new-refresh"}));
        refresh_chatgpt_tokens(&active).await.unwrap();
        let store = load_accounts().unwrap();
        assert!(store.active_account_id.is_none());
        assert!(store.active_account_home.is_none());
        assert!(store.pending_auth_sync_account_id.is_none());
        assert_eq!(
            read_current_auth()
                .unwrap()
                .unwrap()
                .tokens
                .unwrap()
                .refresh_token,
            "new-refresh"
        );
    }

    #[tokio::test]
    async fn stale_marker_never_publishes_over_another_live_identity() {
        let env = AuthTestEnv::new();
        let a = account("a");
        let b = account("b");
        seed_accounts(vec![a.clone(), b.clone()], Some(&a.id));
        sync_account_auth_file(&b).unwrap();
        let before = std::fs::read(get_codex_auth_file().unwrap()).unwrap();
        env.respond(serde_json::json!({"access_token":"new-access","refresh_token":"a-new"}));
        refresh_chatgpt_tokens(&a).await.unwrap();
        assert_eq!(
            super::account_tokens(&get_account(&a.id).unwrap().unwrap())
                .unwrap()
                .1,
            "a-new"
        );
        assert_eq!(
            std::fs::read(get_codex_auth_file().unwrap()).unwrap(),
            before
        );
    }

    #[tokio::test]
    async fn raw_import_cannot_consume_an_unregistered_live_token_while_running() {
        let env = AuthTestEnv::new();
        let live = account("unregistered");
        seed_accounts(vec![], None);
        sync_account_auth_file(&live).unwrap();
        env.running(true);
        assert!(super::create_chatgpt_account_from_refresh_token(
            "imported".into(),
            "unregistered-refresh".into()
        )
        .await
        .is_err());
        assert!(env.requests().is_empty());
    }

    #[tokio::test]
    async fn unverified_live_identity_cannot_share_an_independently_refreshed_token() {
        let env = AuthTestEnv::new();
        let a = account("a");
        seed_accounts(vec![a.clone()], None);
        let mut live = a.clone();
        if let AuthData::ChatGPT {
            id_token,
            account_id,
            ..
        } = &mut live.auth_data
        {
            *id_token = jwt(None, Some(1));
            *account_id = None;
        }
        sync_account_auth_file(&live).unwrap();
        env.running(true);
        assert!(refresh_chatgpt_tokens(&a).await.is_err());
        assert!(env.requests().is_empty());
        assert_eq!(get_account(&a.id).unwrap().unwrap().auth_data, a.auth_data);
    }

    #[tokio::test]
    async fn codex_starting_during_refresh_prevents_live_publication() {
        let env = AuthTestEnv::new();
        let active = account("active");
        seed_accounts(vec![active.clone()], Some(&active.id));
        let before = std::fs::read(get_codex_auth_file().unwrap()).unwrap();
        env.respond(serde_json::json!({"access_token":"new-access","refresh_token":"new-refresh"}));
        env.start_codex_during_refresh();
        refresh_chatgpt_tokens(&active).await.unwrap();
        assert_eq!(
            super::account_tokens(&get_account(&active.id).unwrap().unwrap())
                .unwrap()
                .1,
            "new-refresh"
        );
        assert_eq!(
            std::fs::read(get_codex_auth_file().unwrap()).unwrap(),
            before
        );
        assert!(load_accounts()
            .unwrap()
            .pending_auth_sync_account_id
            .is_none());
    }

    #[test]
    fn refresh_response_preserves_refresh_token_when_server_omits_rotation() {
        let account = StoredAccount::new_chatgpt(
            "ChatGPT".into(),
            None,
            None,
            None,
            id_token("old-account", "person@example.com", "plus"),
            "old-access".into(),
            "old-refresh".into(),
            Some("old-account".into()),
        );
        let original_id_token = match &account.auth_data {
            AuthData::ChatGPT { id_token, .. } => id_token.clone(),
            _ => unreachable!(),
        };
        let (updated, error) = apply_refresh_response(
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
        assert!(error.is_none());
        assert_eq!(id_token, original_id_token);
        assert_eq!(access_token, "new-access");
        assert_eq!(refresh_token, "old-refresh");
        assert_eq!(account_id.as_deref(), Some("old-account"));
    }

    #[test]
    fn active_auth_merge_imports_rotated_tokens_for_the_same_account() {
        let mut account = StoredAccount::new_chatgpt(
            "ChatGPT".into(),
            Some("person@example.com".into()),
            Some("plus".into()),
            None,
            id_token("account-a", "person@example.com", "plus"),
            "old-access".into(),
            "old-refresh".into(),
            Some("account-a".into()),
        );

        let result = merge_auth_file_tokens(
            &mut account,
            TokenData {
                id_token: id_token("account-a", "person@example.com", "pro"),
                access_token: "new-access".into(),
                refresh_token: "new-refresh".into(),
                account_id: Some("account-a".into()),
            },
        );

        assert_eq!(result, AuthFileTokenMerge::Updated);
        assert_eq!(account.plan_type.as_deref(), Some("pro"));
        let AuthData::ChatGPT {
            access_token,
            refresh_token,
            account_id,
            ..
        } = account.auth_data
        else {
            panic!("expected ChatGPT account")
        };
        assert_eq!(access_token, "new-access");
        assert_eq!(refresh_token, "new-refresh");
        assert_eq!(account_id.as_deref(), Some("account-a"));
    }

    #[test]
    fn id_token_subscription_claim_is_only_a_missing_value_fallback() {
        let live_expiry = chrono::DateTime::parse_from_rfc3339("2026-09-27T07:09:59Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let stale_expiry = chrono::DateTime::parse_from_rfc3339("2026-08-27T07:09:59Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let stale_id_token = id_token_with_subscription(
            "account-a",
            "person@example.com",
            "plus",
            Some("2026-08-27T07:09:59Z"),
        );

        let mut account = StoredAccount::new_chatgpt(
            "ChatGPT".into(),
            Some("person@example.com".into()),
            Some("plus".into()),
            Some(live_expiry),
            id_token("account-a", "person@example.com", "plus"),
            "old-access".into(),
            "old-refresh".into(),
            Some("account-a".into()),
        );
        assert_eq!(
            merge_auth_file_tokens(
                &mut account,
                TokenData {
                    id_token: stale_id_token.clone(),
                    access_token: "new-access".into(),
                    refresh_token: "new-refresh".into(),
                    account_id: Some("account-a".into()),
                },
            ),
            AuthFileTokenMerge::Updated
        );
        assert_eq!(account.subscription_expires_at, Some(live_expiry));

        let mut missing = StoredAccount::new_chatgpt(
            "ChatGPT".into(),
            Some("person@example.com".into()),
            Some("plus".into()),
            None,
            id_token("account-a", "person@example.com", "plus"),
            "old-access".into(),
            "old-refresh".into(),
            Some("account-a".into()),
        );
        merge_auth_file_tokens(
            &mut missing,
            TokenData {
                id_token: stale_id_token.clone(),
                access_token: "new-access".into(),
                refresh_token: "new-refresh".into(),
                account_id: Some("account-a".into()),
            },
        );
        assert_eq!(missing.subscription_expires_at, Some(stale_expiry));

        let refreshed = apply_refresh_response(
            account,
            RefreshTokenResponse {
                id_token: Some(stale_id_token),
                access_token: "newer-access".into(),
                refresh_token: Some("newer-refresh".into()),
            },
        )
        .unwrap()
        .0;
        assert_eq!(refreshed.subscription_expires_at, Some(live_expiry));
    }

    #[test]
    fn active_auth_merge_rejects_credentials_for_a_different_account() {
        let original_id_token = id_token("account-a", "first@example.com", "plus");
        let mut account = StoredAccount::new_chatgpt(
            "ChatGPT".into(),
            Some("first@example.com".into()),
            Some("plus".into()),
            None,
            original_id_token.clone(),
            "old-access".into(),
            "old-refresh".into(),
            Some("account-a".into()),
        );

        let result = merge_auth_file_tokens(
            &mut account,
            TokenData {
                id_token: id_token("account-b", "second@example.com", "pro"),
                access_token: "other-access".into(),
                refresh_token: "other-refresh".into(),
                account_id: Some("account-b".into()),
            },
        );

        assert_eq!(result, AuthFileTokenMerge::IdentityMismatch);
        let AuthData::ChatGPT {
            id_token,
            access_token,
            refresh_token,
            account_id,
        } = account.auth_data
        else {
            panic!("expected ChatGPT account")
        };
        assert_eq!(id_token, original_id_token);
        assert_eq!(access_token, "old-access");
        assert_eq!(refresh_token, "old-refresh");
        assert_eq!(account_id.as_deref(), Some("account-a"));
    }
}
