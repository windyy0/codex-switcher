//! Usage query Tauri commands

use crate::api::usage::{
    fetch_chatgpt_account_metadata, get_account_usage, refresh_all_usage,
    warmup_account as send_warmup,
};
use crate::auth::{get_account, load_accounts, refresh_chatgpt_tokens, update_account_metadata};
use crate::commands::account::lock_account_transition;
use crate::types::{AccountInfo, AuthData, UsageInfo, WarmupSummary};
use futures::{stream, StreamExt};
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::Mutex as AsyncMutex;

const ACTIVE_USAGE_CACHE_TTL: Duration = Duration::from_secs(60);
const INACTIVE_USAGE_CACHE_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Clone)]
struct CachedUsage {
    usage: UsageInfo,
    fetched_at: Instant,
}

static USAGE_CACHE: LazyLock<Mutex<HashMap<String, CachedUsage>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static USAGE_FETCH_LOCKS: LazyLock<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Fetch usage info for a specific account (shared by the Tauri command and web mode).
pub async fn fetch_usage(account_id: &str) -> Result<UsageInfo, String> {
    let account = get_account(account_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Account not found: {account_id}"))?;

    get_account_usage(&account).await.map_err(|e| e.to_string())
}

fn usage_cache_ttl(account_id: &str) -> Duration {
    let is_active = load_accounts()
        .ok()
        .and_then(|store| store.active_account_id)
        .as_deref()
        == Some(account_id);
    if is_active {
        ACTIVE_USAGE_CACHE_TTL
    } else {
        INACTIVE_USAGE_CACHE_TTL
    }
}

fn cached_usage(account_id: &str) -> Option<UsageInfo> {
    let ttl = usage_cache_ttl(account_id);
    USAGE_CACHE
        .lock()
        .ok()
        .and_then(|cache| cache.get(account_id).cloned())
        .filter(|cached| cached.fetched_at.elapsed() < ttl)
        .map(|cached| cached.usage)
}

fn account_fetch_lock(account_id: &str) -> Arc<AsyncMutex<()>> {
    let mut locks = USAGE_FETCH_LOCKS
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    locks
        .entry(account_id.to_string())
        .or_insert_with(|| Arc::new(AsyncMutex::new(())))
        .clone()
}

/// Return a shared backend snapshot. Active accounts refresh at most once per
/// minute; inactive accounts refresh at most once every 30 minutes.
pub async fn fetch_usage_cached(
    account_id: &str,
    force_refresh: bool,
) -> Result<UsageInfo, String> {
    if !force_refresh {
        if let Some(usage) = cached_usage(account_id) {
            return Ok(usage);
        }
    }

    let fetch_lock = account_fetch_lock(account_id);
    let _guard = fetch_lock.lock().await;
    if !force_refresh {
        if let Some(usage) = cached_usage(account_id) {
            return Ok(usage);
        }
    }

    let usage = fetch_usage(account_id).await?;
    if usage.error.is_none() {
        if let Ok(mut cache) = USAGE_CACHE.lock() {
            cache.insert(
                account_id.to_string(),
                CachedUsage {
                    usage: usage.clone(),
                    fetched_at: Instant::now(),
                },
            );
        }
    }
    Ok(usage)
}

/// Get usage info for a specific account
#[tauri::command]
pub async fn get_usage(
    app: tauri::AppHandle,
    account_id: String,
    force_refresh: Option<bool>,
) -> Result<UsageInfo, String> {
    let usage = fetch_usage_cached(&account_id, force_refresh.unwrap_or(false)).await?;

    // Keep the tray menu/title in sync with whichever UI fetched fresh usage.
    #[cfg(desktop)]
    crate::tray::ingest_usage(&app, vec![usage.clone()]);
    #[cfg(not(desktop))]
    let _ = app;

    Ok(usage)
}

/// Force-refresh account metadata for a specific account.
/// For ChatGPT accounts this refreshes OAuth tokens and pulls live subscription metadata.
/// For API key accounts this is a no-op.
#[tauri::command]
pub async fn refresh_account_metadata(account_id: String) -> Result<AccountInfo, String> {
    let account = get_account(&account_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Account not found: {account_id}"))?;

    let updated = match &account.auth_data {
        AuthData::ApiKey { .. } => account,
        AuthData::ChatGPT { .. } => {
            let refreshed = refresh_chatgpt_tokens(&account)
                .await
                .map_err(|e| e.to_string())?;
            let live_metadata = fetch_chatgpt_account_metadata(&refreshed)
                .await
                .map_err(|e| e.to_string())?;

            let _transition_guard = lock_account_transition()?;
            update_account_metadata(
                &account_id,
                None,
                None,
                live_metadata.plan_type,
                Some(live_metadata.subscription_expires_at),
            )
            .map_err(|e| e.to_string())?
        }
    };

    let store = load_accounts().map_err(|e| e.to_string())?;
    let active_id = store.active_account_id.as_deref();
    Ok(AccountInfo::from_stored(&updated, active_id))
}

/// Refresh usage info for all accounts
#[tauri::command]
pub async fn refresh_all_accounts_usage() -> Result<Vec<UsageInfo>, String> {
    let store = load_accounts().map_err(|e| e.to_string())?;
    Ok(refresh_all_usage(&store.accounts).await)
}

/// Send a minimal warm-up request for one account
#[tauri::command]
pub async fn warmup_account(account_id: String) -> Result<(), String> {
    let account = get_account(&account_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Account not found: {account_id}"))?;

    send_warmup(&account).await.map_err(|e| e.to_string())
}

/// Send minimal warm-up requests for all accounts
#[tauri::command]
pub async fn warmup_all_accounts() -> Result<WarmupSummary, String> {
    let store = load_accounts().map_err(|e| e.to_string())?;
    let eligible_accounts = store
        .accounts
        .into_iter()
        .filter(|account| matches!(account.auth_data, AuthData::ChatGPT { .. }))
        .collect::<Vec<_>>();
    let total_accounts = eligible_accounts.len();
    let concurrency = total_accounts.min(10).max(1);

    let results: Vec<(String, bool)> = stream::iter(eligible_accounts)
        .map(|account| async move {
            let account_id = account.id.clone();
            let failed = send_warmup(&account).await.is_err();
            (account_id, failed)
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;

    let failed_account_ids = results
        .into_iter()
        .filter_map(|(account_id, failed)| failed.then_some(account_id))
        .collect::<Vec<_>>();

    let warmed_accounts = total_accounts.saturating_sub(failed_account_ids.len());
    Ok(WarmupSummary {
        total_accounts,
        warmed_accounts,
        failed_account_ids,
    })
}
