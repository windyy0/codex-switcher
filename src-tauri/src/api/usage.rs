//! Usage API client for fetching rate limits and credits

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use futures::{stream, StreamExt};
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, USER_AGENT},
    StatusCode,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::auth::{ensure_chatgpt_tokens_fresh, refresh_chatgpt_tokens};
use crate::types::{
    AuthData, CreditStatusDetails, RateLimitDetails, RateLimitStatusPayload, RateLimitWindow,
    StoredAccount, UsageInfo,
};

const CHATGPT_BACKEND_API: &str = "https://chatgpt.com/backend-api";
const CHATGPT_ACCOUNTS_CHECK_API: &str =
    "https://chatgpt.com/backend-api/accounts/check/v4-2023-04-27";
const CHATGPT_CODEX_RESPONSES_API: &str = "https://chatgpt.com/backend-api/codex/responses";
const SESSION_WINDOW_SECONDS: i32 = 5 * 60 * 60;
const WEEKLY_WINDOW_SECONDS: i32 = 7 * 24 * 60 * 60;
const CODEX_USER_AGENT: &str = "codex-cli/1.0.0";

#[derive(Debug, Clone)]
pub struct ChatGptAccountMetadata {
    pub plan_type: Option<String>,
    pub subscription_expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct AccountsCheckResponse {
    #[serde(default)]
    accounts: HashMap<String, AccountsCheckEntry>,
}

#[derive(Debug, Deserialize)]
struct AccountsCheckEntry {
    #[serde(default)]
    account: Option<AccountsCheckAccount>,
    #[serde(default)]
    entitlement: Option<AccountsCheckEntitlement>,
}

#[derive(Debug, Deserialize)]
struct AccountsCheckAccount {
    #[serde(default)]
    plan_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AccountsCheckEntitlement {
    #[serde(default)]
    expires_at: Option<DateTime<Utc>>,
}

/// Get usage information for an account
pub async fn get_account_usage(account: &StoredAccount) -> Result<UsageInfo> {
    if account.disabled {
        anyhow::bail!("Account is disabled");
    }
    println!("[Usage] Fetching usage for account: {}", account.name);

    match &account.auth_data {
        AuthData::ApiKey { .. } => {
            println!("[Usage] API key accounts don't support usage info");
            Ok(UsageInfo {
                account_id: account.id.clone(),
                plan_type: Some("api_key".to_string()),
                primary_used_percent: None,
                primary_window_minutes: None,
                primary_resets_at: None,
                secondary_used_percent: None,
                secondary_window_minutes: None,
                secondary_resets_at: None,
                has_credits: None,
                unlimited_credits: None,
                credits_balance: None,
                error: Some("Usage info not available for API key accounts".to_string()),
            })
        }
        AuthData::ChatGPT { .. } => get_usage_with_chatgpt_auth(account).await,
    }
}

/// Send a minimal authenticated request to warm up account traffic paths.
pub async fn warmup_account(account: &StoredAccount) -> Result<()> {
    if account.disabled {
        anyhow::bail!("Account is disabled");
    }
    println!(
        "[Warmup] Sending warm-up request for account: {}",
        account.name
    );

    match &account.auth_data {
        // An API-key account without a per-account fragment can still inherit a
        // third-party provider from the user's normal config.toml. We cannot
        // prove the key belongs to OpenAI here, so never send it to a fixed host.
        AuthData::ApiKey { .. } => anyhow::bail!("Warm-up is disabled for API key accounts"),
        AuthData::ChatGPT { .. } => warmup_with_chatgpt_auth(account).await,
    }
}

pub async fn fetch_chatgpt_account_metadata(
    account: &StoredAccount,
) -> Result<ChatGptAccountMetadata> {
    let (access_token, chatgpt_account_id) = extract_chatgpt_auth(account)?;
    let response =
        send_chatgpt_get_request(CHATGPT_ACCOUNTS_CHECK_API, access_token, chatgpt_account_id)
            .await?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Accounts check API error: {status} - {body}");
    }

    let payload: AccountsCheckResponse = response
        .json()
        .await
        .context("Failed to parse accounts check response")?;

    let selected_entry = chatgpt_account_id
        .and_then(|account_id| payload.accounts.get(account_id))
        .or_else(|| payload.accounts.get("default"))
        .or_else(|| payload.accounts.values().next())
        .context("Accounts check response did not include an account entry")?;

    Ok(ChatGptAccountMetadata {
        plan_type: selected_entry
            .account
            .as_ref()
            .and_then(|account| account.plan_type.clone()),
        subscription_expires_at: selected_entry
            .entitlement
            .as_ref()
            .and_then(|entitlement| entitlement.expires_at),
    })
}

async fn get_usage_with_chatgpt_auth(account: &StoredAccount) -> Result<UsageInfo> {
    let fresh_account = ensure_chatgpt_tokens_fresh(account).await?;
    let (access_token, chatgpt_account_id) = extract_chatgpt_auth(&fresh_account)?;

    let response = send_chatgpt_usage_request(access_token, chatgpt_account_id).await?;
    if response.status() == StatusCode::UNAUTHORIZED {
        println!(
            "[Usage] Unauthorized for account {}, refreshing token and retrying once",
            fresh_account.name
        );
        let refreshed_account = refresh_chatgpt_tokens(&fresh_account).await?;
        let (retry_token, retry_account_id) = extract_chatgpt_auth(&refreshed_account)?;
        let retry_response = send_chatgpt_usage_request(retry_token, retry_account_id).await?;
        return parse_usage_response(
            &refreshed_account.id,
            &refreshed_account.name,
            retry_response,
        )
        .await;
    }

    parse_usage_response(&fresh_account.id, &fresh_account.name, response).await
}

async fn parse_usage_response(
    account_id: &str,
    account_name: &str,
    response: reqwest::Response,
) -> Result<UsageInfo> {
    let status = response.status();
    println!("[Usage] Response status: {status}");

    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        println!("[Usage] Error response: {body}");
        return Ok(UsageInfo::error(
            account_id.to_string(),
            format!("API error: {status}"),
        ));
    }

    let body_text = response
        .text()
        .await
        .context("Failed to read response body")?;
    println!(
        "[Usage] Response body: {}",
        &body_text[..body_text.len().min(200)]
    );

    let payload: RateLimitStatusPayload =
        serde_json::from_str(&body_text).context("Failed to parse usage response")?;

    println!("[Usage] Parsed plan_type: {}", payload.plan_type);

    let usage = convert_payload_to_usage_info(account_id, payload);
    println!(
        "[Usage] {} - primary: {:?}%, plan: {:?}",
        account_name, usage.primary_used_percent, usage.plan_type
    );

    Ok(usage)
}

async fn warmup_with_chatgpt_auth(account: &StoredAccount) -> Result<()> {
    let fresh_account = ensure_chatgpt_tokens_fresh(account).await?;
    let (access_token, chatgpt_account_id) = extract_chatgpt_auth(&fresh_account)?;

    let mut response = send_chatgpt_warmup_request(access_token, chatgpt_account_id, true).await?;
    if response.status() == StatusCode::UNAUTHORIZED {
        println!(
            "[Warmup] Unauthorized for account {}, refreshing token and retrying once",
            fresh_account.name
        );
        let refreshed_account = refresh_chatgpt_tokens(&fresh_account).await?;
        let (retry_token, retry_account_id) = extract_chatgpt_auth(&refreshed_account)?;
        response = send_chatgpt_warmup_request(retry_token, retry_account_id, true).await?;
    }

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        println!("[Warmup] ChatGPT warm-up error response: {body}");
        anyhow::bail!(format_warmup_http_error(status, &body));
    }

    let body = response.text().await.unwrap_or_default();
    log_warmup_response("ChatGPT", &body, true);

    Ok(())
}

fn build_warmup_payload(stream: bool, include_max_output_tokens: bool) -> serde_json::Value {
    let mut payload = json!({
        "model": "gpt-5.4-mini",
        "instructions": "You are Codex.",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [
                    {
                        "type": "input_text",
                        "text": "Hi"
                    }
                ]
            }
        ],
        "tools": [],
        "tool_choice": "auto",
        "parallel_tool_calls": false,
        "reasoning": {
            "effort": "low"
        },
        "store": false,
        "stream": stream
    });

    if include_max_output_tokens {
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("max_output_tokens".to_string(), json!(1));
        }
    }

    payload
}

fn build_chatgpt_headers(
    access_token: &str,
    chatgpt_account_id: Option<&str>,
) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(CODEX_USER_AGENT));
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {access_token}")).context("Invalid access token")?,
    );

    if let Some(acc_id) = chatgpt_account_id {
        println!("[Usage] Using ChatGPT Account ID: {acc_id}");
        if let Ok(header_name) = HeaderName::from_bytes(b"chatgpt-account-id") {
            if let Ok(header_value) = HeaderValue::from_str(acc_id) {
                headers.insert(header_name, header_value);
            }
        }
    }

    Ok(headers)
}

fn extract_chatgpt_auth(account: &StoredAccount) -> Result<(&str, Option<&str>)> {
    match &account.auth_data {
        AuthData::ChatGPT {
            access_token,
            account_id,
            ..
        } => Ok((access_token.as_str(), account_id.as_deref())),
        AuthData::ApiKey { .. } => anyhow::bail!("Account is not using ChatGPT OAuth"),
    }
}

async fn send_chatgpt_usage_request(
    access_token: &str,
    chatgpt_account_id: Option<&str>,
) -> Result<reqwest::Response> {
    send_chatgpt_get_request(
        &format!("{CHATGPT_BACKEND_API}/wham/usage"),
        access_token,
        chatgpt_account_id,
    )
    .await
}

async fn send_chatgpt_get_request(
    url: &str,
    access_token: &str,
    chatgpt_account_id: Option<&str>,
) -> Result<reqwest::Response> {
    let client = reqwest::Client::new();
    let headers = build_chatgpt_headers(access_token, chatgpt_account_id)?;
    println!("[Usage] Requesting: {url}");

    client
        .get(url)
        .headers(headers)
        .send()
        .await
        .with_context(|| format!("Failed to send GET request to {url}"))
}

async fn send_chatgpt_warmup_request(
    access_token: &str,
    chatgpt_account_id: Option<&str>,
    stream: bool,
) -> Result<reqwest::Response> {
    let client = reqwest::Client::new();
    let headers = build_chatgpt_headers(access_token, chatgpt_account_id)?;
    let payload = build_warmup_payload(stream, false);

    client
        .post(CHATGPT_CODEX_RESPONSES_API)
        .headers(headers)
        .json(&payload)
        .send()
        .await
        .context("Failed to send ChatGPT warm-up request")
}

fn log_warmup_response(source: &str, body: &str, is_sse: bool) {
    if body.trim().is_empty() {
        println!("[Warmup] {source} warm-up response was empty");
        return;
    }

    let preview = truncate_text(body, 300);
    println!("[Warmup] {source} warm-up response preview: {preview}");

    let extracted = if is_sse {
        extract_text_from_sse(body)
    } else {
        extract_text_from_json(body)
    };

    if let Some(message) = extracted {
        let message_preview = truncate_text(&message, 200);
        println!("[Warmup] {source} warm-up message: {message_preview}");
    }
}

fn truncate_text(text: &str, max_len: usize) -> String {
    if text.chars().count() <= max_len {
        return text.to_string();
    }
    let mut out = text.chars().take(max_len).collect::<String>();
    out.push_str("...");
    out
}

fn format_warmup_http_error(status: StatusCode, body: &str) -> String {
    let detail = extract_warmup_error_detail(body);
    match detail {
        Some(detail) => format!("ChatGPT warm-up failed with status {status}: {detail}"),
        None => format!("ChatGPT warm-up failed with status {status}"),
    }
}

fn extract_warmup_error_detail(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        let error = value.get("error").unwrap_or(&value);
        let code = error
            .get("code")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty());
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty());

        let detail = match (code, message) {
            (Some(code), Some(message)) if !message.contains(code) => {
                format!("{code}: {message}")
            }
            (_, Some(message)) => message.to_string(),
            (Some(code), None) => code.to_string(),
            (None, None) => return Some(truncate_text(trimmed, 240)),
        };
        return Some(truncate_text(&detail, 240));
    }

    Some(truncate_text(
        trimmed.lines().next().unwrap_or(trimmed),
        240,
    ))
}

fn extract_text_from_sse(body: &str) -> Option<String> {
    let mut last_text: Option<String> = None;
    for line in body.lines() {
        let line = line.trim();
        if !line.starts_with("data:") {
            continue;
        }
        let data = line.trim_start_matches("data:").trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(data) {
            if let Some(text) = extract_last_text_from_value(&value) {
                last_text = Some(text);
            }
        }
    }
    last_text.filter(|text| !text.trim().is_empty())
}

fn extract_text_from_json(body: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(body).ok()?;
    extract_last_text_from_value(&value)
}

fn extract_last_text_from_value(value: &Value) -> Option<String> {
    let mut last: Option<String> = None;
    collect_last_text(value, &mut last);
    last
}

fn collect_last_text(value: &Value, last: &mut Option<String>) {
    match value {
        Value::Object(map) => {
            for (key, val) in map {
                if matches!(key.as_str(), "text" | "delta" | "output_text") {
                    if let Value::String(text) = val {
                        if !text.is_empty() {
                            *last = Some(text.clone());
                        }
                    }
                }
                collect_last_text(val, last);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_last_text(item, last);
            }
        }
        _ => {}
    }
}

/// Convert API response to UsageInfo
fn convert_payload_to_usage_info(account_id: &str, payload: RateLimitStatusPayload) -> UsageInfo {
    let (primary, secondary) = extract_rate_limits(payload.rate_limit);
    let credits = extract_credits(payload.credits);

    UsageInfo {
        account_id: account_id.to_string(),
        plan_type: Some(payload.plan_type),
        primary_used_percent: primary.as_ref().map(|w| w.used_percent),
        primary_window_minutes: primary
            .as_ref()
            .and_then(|w| w.limit_window_seconds)
            .map(|s| (i64::from(s) + 59) / 60),
        primary_resets_at: primary.as_ref().and_then(|w| w.reset_at),
        secondary_used_percent: secondary.as_ref().map(|w| w.used_percent),
        secondary_window_minutes: secondary
            .as_ref()
            .and_then(|w| w.limit_window_seconds)
            .map(|s| (i64::from(s) + 59) / 60),
        secondary_resets_at: secondary.as_ref().and_then(|w| w.reset_at),
        has_credits: credits.as_ref().map(|c| c.has_credits),
        unlimited_credits: credits.as_ref().map(|c| c.unlimited),
        credits_balance: credits.and_then(|c| c.balance),
        error: None,
    }
}

fn extract_rate_limits(
    rate_limit: Option<RateLimitDetails>,
) -> (Option<RateLimitWindow>, Option<RateLimitWindow>) {
    let Some(details) = rate_limit else {
        return (None, None);
    };

    match (details.primary_window, details.secondary_window) {
        // The backend can omit the 5-hour window and promote the weekly window
        // into primary_window. Keep UsageInfo semantic so consumers still treat
        // primary as session and secondary as weekly.
        (Some(primary), None) if is_weekly_window(&primary) => (None, Some(primary)),
        (None, Some(secondary)) if is_session_window(&secondary) => (Some(secondary), None),
        (Some(primary), Some(secondary))
            if is_weekly_window(&primary) && is_session_window(&secondary) =>
        {
            (Some(secondary), Some(primary))
        }
        (primary, secondary) => (primary, secondary),
    }
}

fn is_session_window(window: &RateLimitWindow) -> bool {
    window.limit_window_seconds == Some(SESSION_WINDOW_SECONDS)
}

fn is_weekly_window(window: &RateLimitWindow) -> bool {
    window.limit_window_seconds == Some(WEEKLY_WINDOW_SECONDS)
}

fn extract_credits(credits: Option<CreditStatusDetails>) -> Option<CreditStatusDetails> {
    credits
}

/// Refresh all account usage
pub async fn refresh_all_usage(accounts: &[StoredAccount]) -> Vec<UsageInfo> {
    let eligible_accounts = accounts
        .iter()
        .filter(|account| {
            !account.disabled && matches!(account.auth_data, AuthData::ChatGPT { .. })
        })
        .cloned()
        .collect::<Vec<_>>();
    println!(
        "[Usage] Refreshing usage for {} ChatGPT accounts",
        eligible_accounts.len()
    );

    let concurrency = eligible_accounts.len().min(10).max(1);
    let results: Vec<UsageInfo> = stream::iter(eligible_accounts)
        .map(|account| async move {
            match get_account_usage(&account).await {
                Ok(info) => info,
                Err(e) => {
                    println!("[Usage] Error for {}: {}", account.name, e);
                    UsageInfo::error(account.id.clone(), e.to_string())
                }
            }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;

    println!("[Usage] Refresh complete");
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disabled_chatgpt_account() -> StoredAccount {
        let mut account = StoredAccount::new_chatgpt(
            "Archived".into(),
            None,
            None,
            None,
            "id-token".into(),
            "access-token".into(),
            "refresh-token".into(),
            None,
        );
        account.disabled = true;
        account
    }

    fn rate_limit_window(used_percent: f64, window_seconds: i32) -> RateLimitWindow {
        RateLimitWindow {
            used_percent,
            limit_window_seconds: Some(window_seconds),
            reset_at: Some(1_800_000_000),
        }
    }

    #[test]
    fn keeps_legacy_session_and_weekly_windows_in_place() {
        let (session, weekly) = extract_rate_limits(Some(RateLimitDetails {
            primary_window: Some(rate_limit_window(27.0, SESSION_WINDOW_SECONDS)),
            secondary_window: Some(rate_limit_window(82.0, WEEKLY_WINDOW_SECONDS)),
        }));

        assert_eq!(
            session.and_then(|window| window.limit_window_seconds),
            Some(SESSION_WINDOW_SECONDS)
        );
        assert_eq!(
            weekly.and_then(|window| window.limit_window_seconds),
            Some(WEEKLY_WINDOW_SECONDS)
        );
    }

    #[test]
    fn moves_weekly_only_primary_window_to_weekly_slot() {
        let (session, weekly) = extract_rate_limits(Some(RateLimitDetails {
            primary_window: Some(rate_limit_window(35.0, WEEKLY_WINDOW_SECONDS)),
            secondary_window: None,
        }));

        assert!(session.is_none());
        assert_eq!(weekly.map(|window| window.used_percent), Some(35.0));
    }

    #[test]
    fn restores_semantic_order_if_backend_reverses_windows() {
        let (session, weekly) = extract_rate_limits(Some(RateLimitDetails {
            primary_window: Some(rate_limit_window(82.0, WEEKLY_WINDOW_SECONDS)),
            secondary_window: Some(rate_limit_window(27.0, SESSION_WINDOW_SECONDS)),
        }));

        assert_eq!(session.map(|window| window.used_percent), Some(27.0));
        assert_eq!(weekly.map(|window| window.used_percent), Some(82.0));
    }

    #[test]
    fn preserves_unknown_windows_by_backend_position() {
        let (primary, secondary) = extract_rate_limits(Some(RateLimitDetails {
            primary_window: Some(rate_limit_window(11.0, 60 * 60)),
            secondary_window: Some(rate_limit_window(22.0, 30 * 24 * 60 * 60)),
        }));

        assert_eq!(primary.map(|window| window.used_percent), Some(11.0));
        assert_eq!(secondary.map(|window| window.used_percent), Some(22.0));
    }

    #[tokio::test]
    async fn disabled_accounts_are_rejected_before_network_requests() {
        let account = disabled_chatgpt_account();

        let usage_error = get_account_usage(&account).await.unwrap_err();
        let warmup_error = warmup_account(&account).await.unwrap_err();

        assert_eq!(usage_error.to_string(), "Account is disabled");
        assert_eq!(warmup_error.to_string(), "Account is disabled");
    }

    #[test]
    fn warmup_http_errors_include_model_failure_details() {
        let error = format_warmup_http_error(
            StatusCode::BAD_REQUEST,
            r#"{"error":{"code":"model_not_found","message":"The requested model is unavailable."}}"#,
        );

        assert_eq!(
            error,
            "ChatGPT warm-up failed with status 400 Bad Request: model_not_found: The requested model is unavailable."
        );
    }

    #[test]
    fn warmup_http_error_details_are_unicode_safe_and_bounded() {
        let detail = "模型不可用".repeat(100);
        let error = format_warmup_http_error(StatusCode::NOT_FOUND, &detail);

        assert!(error.starts_with("ChatGPT warm-up failed with status 404 Not Found: 模型不可用"));
        assert!(error.ends_with("..."));
    }
}
