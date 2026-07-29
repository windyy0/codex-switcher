//! Usage query Tauri commands

use crate::account_health::{classify_error_message, healthy_observation};
use crate::api::usage::{
    fetch_chatgpt_account_metadata, get_account_usage, warmup_account as send_warmup,
};
use crate::auth::{
    get_account, load_accounts, record_account_health, refresh_chatgpt_tokens,
    update_account_metadata,
};
use crate::commands::account::lock_account_transition;
use crate::types::{
    AccountHealthObservation, AccountHealthSource, AccountInfo, AuthData, UsageInfo, WarmupFailure,
    WarmupSummary,
};
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

pub fn clear_usage_cache(account_id: &str) {
    if let Ok(mut cache) = USAGE_CACHE.lock() {
        cache.remove(account_id);
    }
}

/// Fetch usage info for a specific account (shared by the Tauri command and web mode).
pub async fn fetch_usage(account_id: &str) -> Result<UsageInfo, String> {
    let account = get_account(account_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Account not found: {account_id}"))?;
    if account.disabled {
        return Err("Account is disabled".to_string());
    }

    match get_account_usage(&account).await {
        Ok(mut usage) => {
            let observation = usage
                .health_observation
                .take()
                .unwrap_or_else(|| healthy_observation(AccountHealthSource::Usage));
            persist_health_observation(account_id, observation);
            Ok(usage)
        }
        Err(error) => {
            persist_health_observation(
                account_id,
                classify_error_message(AccountHealthSource::Usage, &error.to_string()),
            );
            Err(error.to_string())
        }
    }
}

fn persist_health_observation(account_id: &str, observation: AccountHealthObservation) {
    let result = (|| {
        let _transition_guard = lock_account_transition()?;
        record_account_health(account_id, observation).map_err(|error| error.to_string())?;
        Ok::<(), String>(())
    })();
    if let Err(error) = result {
        eprintln!("[Health] Failed to record account {account_id} status: {error}");
    }
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
    let account = get_account(account_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Account not found: {account_id}"))?;
    if account.disabled {
        return Err("Account is disabled".to_string());
    }

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
    if account.disabled {
        return Err("Account is disabled".to_string());
    }

    let updated = match &account.auth_data {
        AuthData::ApiKey { .. } => account,
        AuthData::ChatGPT { .. } => {
            let refreshed = match refresh_chatgpt_tokens(&account).await {
                Ok(refreshed) => refreshed,
                Err(error) => {
                    persist_health_observation(
                        &account_id,
                        classify_error_message(
                            AccountHealthSource::TokenRefresh,
                            &error.to_string(),
                        ),
                    );
                    return Err(error.to_string());
                }
            };
            let live_metadata = match fetch_chatgpt_account_metadata(&refreshed).await {
                Ok(metadata) => metadata,
                Err(error) => {
                    persist_health_observation(
                        &account_id,
                        classify_error_message(
                            AccountHealthSource::AccountsCheck,
                            &error.to_string(),
                        ),
                    );
                    return Err(error.to_string());
                }
            };

            let _transition_guard = lock_account_transition()?;
            let updated = update_account_metadata(
                &account_id,
                None,
                None,
                live_metadata.plan_type,
                Some(live_metadata.subscription_expires_at),
            )
            .map_err(|e| e.to_string())?;
            record_account_health(
                &account_id,
                healthy_observation(AccountHealthSource::AccountsCheck),
            )
            .map_err(|error| error.to_string())?;
            updated
        }
    };

    let store = load_accounts().map_err(|e| e.to_string())?;
    let active_id = store.active_account_id.as_deref();
    let latest = store
        .accounts
        .iter()
        .find(|candidate| candidate.id == updated.id)
        .unwrap_or(&updated);
    Ok(AccountInfo::from_stored(latest, active_id))
}

/// Refresh usage info for all accounts
#[tauri::command]
pub async fn refresh_all_accounts_usage() -> Result<Vec<UsageInfo>, String> {
    let store = load_accounts().map_err(|e| e.to_string())?;
    let eligible_account_ids = store
        .accounts
        .into_iter()
        .filter(|account| {
            !account.disabled
                && !account.health_blocks_account_actions()
                && matches!(account.auth_data, AuthData::ChatGPT { .. })
        })
        .map(|account| account.id)
        .collect::<Vec<_>>();
    let concurrency = eligible_account_ids.len().min(10).max(1);
    Ok(stream::iter(eligible_account_ids)
        .map(|account_id| async move {
            match fetch_usage(&account_id).await {
                Ok(usage) => usage,
                Err(error) => UsageInfo::error(account_id, error),
            }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await)
}

/// Send a minimal warm-up request for one account
#[tauri::command]
pub async fn warmup_account(account_id: String) -> Result<(), String> {
    let account = get_account(&account_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Account not found: {account_id}"))?;
    if account.disabled {
        return Err("Account is disabled".to_string());
    }
    if account.health_blocks_account_actions() {
        return Err("Account authentication or availability requires attention".to_string());
    }

    send_warmup(&account).await.map_err(|e| e.to_string())
}

/// Send minimal warm-up requests for all accounts
#[tauri::command]
pub async fn warmup_all_accounts() -> Result<WarmupSummary, String> {
    let store = load_accounts().map_err(|e| e.to_string())?;
    let eligible_accounts = store
        .accounts
        .into_iter()
        .filter(|account| {
            !account.disabled
                && !account.health_blocks_account_actions()
                && matches!(account.auth_data, AuthData::ChatGPT { .. })
        })
        .collect::<Vec<_>>();
    let total_accounts = eligible_accounts.len();
    let concurrency = total_accounts.min(10).max(1);

    let results: Vec<(String, Option<String>)> = stream::iter(eligible_accounts)
        .map(|account| async move {
            let account_id = account.id.clone();
            let error = send_warmup(&account)
                .await
                .err()
                .map(|error| error.to_string());
            (account_id, error)
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;

    let failed_accounts = results
        .into_iter()
        .filter_map(|(account_id, error)| error.map(|error| WarmupFailure { account_id, error }))
        .collect::<Vec<_>>();
    let failed_account_ids = failed_accounts
        .iter()
        .map(|failure| failure.account_id.clone())
        .collect::<Vec<_>>();

    let warmed_accounts = total_accounts.saturating_sub(failed_accounts.len());
    Ok(WarmupSummary {
        total_accounts,
        warmed_accounts,
        failed_account_ids,
        failed_accounts,
    })
}
