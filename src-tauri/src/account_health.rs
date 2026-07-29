use chrono::{Duration, Utc};
use serde_json::Value;

use crate::types::{
    AccountHealthDiagnostic, AccountHealthObservation, AccountHealthSource, AccountHealthStatus,
    StoredAccount,
};

const MAX_HEALTH_HISTORY: usize = 20;
const HEALTH_HISTORY_RETENTION_DAYS: i64 = 30;
const MAX_HEALTH_MESSAGE_CHARS: usize = 500;

const REAUTH_CODES: &[&str] = &[
    "token_invalidated",
    "token_expired",
    "token_revoked",
    "invalid_token",
    "refresh_token_reused",
    "refresh_token_invalidated",
    "refresh_token_expired",
    "invalid_grant",
    "app_session_terminated",
    "unauthorized_client",
];

const ACCOUNT_DEACTIVATED_CODES: &[&str] = &["account_deactivated"];
const WORKSPACE_DEACTIVATED_CODES: &[&str] = &["workspace_deactivated", "deactivated_workspace"];
const LIMITED_CODES: &[&str] = &[
    "usage_limit_reached",
    "usage_limit_exceeded",
    "usage_limit_exhausted",
    "rate_limit_exceeded",
];

pub fn healthy_observation(source: AccountHealthSource) -> AccountHealthObservation {
    AccountHealthObservation {
        status: AccountHealthStatus::Healthy,
        source,
        http_status: Some(200),
        error_code: None,
        message: None,
    }
}

pub fn classify_http_error(
    source: AccountHealthSource,
    http_status: u16,
    body: &str,
) -> AccountHealthObservation {
    let (structured_code, structured_message) = extract_error_fields(body);
    let code = structured_code.or_else(|| find_known_code(body).map(str::to_string));
    let message = structured_message.or_else(|| sanitize_message(body));
    classify_signal(source, Some(http_status), code, message)
}

pub fn classify_error_message(
    default_source: AccountHealthSource,
    error: &str,
) -> AccountHealthObservation {
    let source = if error.to_ascii_lowercase().contains("token refresh") {
        AccountHealthSource::TokenRefresh
    } else {
        default_source
    };
    let code = find_known_code(error).map(str::to_string);
    let http_status = extract_http_status(error);
    let message = sanitize_message(error);
    classify_signal(source, http_status, code, message)
}

fn classify_signal(
    source: AccountHealthSource,
    http_status: Option<u16>,
    error_code: Option<String>,
    message: Option<String>,
) -> AccountHealthObservation {
    let normalized_code = error_code
        .as_deref()
        .map(str::trim)
        .filter(|code| !code.is_empty())
        .map(str::to_ascii_lowercase);
    let normalized_message = message.as_deref().unwrap_or_default().to_ascii_lowercase();

    let status = if matches_code(normalized_code.as_deref(), WORKSPACE_DEACTIVATED_CODES)
        || contains_exact_deactivation_phrase(&normalized_message, true)
    {
        AccountHealthStatus::WorkspaceDeactivated
    } else if matches_code(normalized_code.as_deref(), ACCOUNT_DEACTIVATED_CODES)
        || contains_exact_deactivation_phrase(&normalized_message, false)
    {
        AccountHealthStatus::AccountDeactivated
    } else if matches_code(normalized_code.as_deref(), REAUTH_CODES)
        || http_status == Some(401)
        || (source == AccountHealthSource::TokenRefresh && matches!(http_status, Some(400 | 401)))
    {
        AccountHealthStatus::ReauthRequired
    } else if matches_code(normalized_code.as_deref(), LIMITED_CODES) || http_status == Some(429) {
        AccountHealthStatus::Limited
    } else if matches!(http_status, Some(403 | 408 | 500..=599))
        || normalized_message.contains("timed out")
        || normalized_message.contains("timeout")
        || normalized_message.contains("connection")
        || normalized_message.contains("dns")
        || normalized_message.contains("failed to send")
    {
        AccountHealthStatus::TransientError
    } else {
        AccountHealthStatus::Unknown
    };

    AccountHealthObservation {
        status,
        source,
        http_status,
        error_code: normalized_code,
        message,
    }
}

fn contains_exact_deactivation_phrase(message: &str, workspace: bool) -> bool {
    let phrases: &[&str] = if workspace {
        &[
            "workspace_deactivated",
            "deactivated_workspace",
            "workspace has been deactivated",
            "workspace is deactivated",
            "deactivated workspace",
        ]
    } else {
        &[
            "account_deactivated",
            "account has been deactivated",
            "account is deactivated",
        ]
    };
    phrases.iter().any(|phrase| message.contains(phrase))
}

fn matches_code(code: Option<&str>, candidates: &[&str]) -> bool {
    code.is_some_and(|code| candidates.iter().any(|candidate| code == *candidate))
}

fn find_known_code(value: &str) -> Option<&'static str> {
    let normalized = value.to_ascii_lowercase();
    WORKSPACE_DEACTIVATED_CODES
        .iter()
        .chain(ACCOUNT_DEACTIVATED_CODES)
        .chain(REAUTH_CODES)
        .chain(LIMITED_CODES)
        .copied()
        .find(|code| normalized.contains(code))
}

fn extract_error_fields(body: &str) -> (Option<String>, Option<String>) {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return (None, None);
    };

    let code = [
        "/error/code",
        "/code",
        "/identity_error_code",
        "/error/type",
    ]
    .iter()
    .find_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
    .or_else(|| value.get("error").and_then(Value::as_str))
    .map(str::to_ascii_lowercase);

    let message = [
        "/error/message",
        "/message",
        "/detail",
        "/error_description",
    ]
    .iter()
    .find_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
    .and_then(sanitize_message);

    (code, message)
}

fn extract_http_status(error: &str) -> Option<u16> {
    error
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| part.len() == 3)
        .filter_map(|part| part.parse::<u16>().ok())
        .find(|status| (400..=599).contains(status))
}

fn sanitize_message(message: &str) -> Option<String> {
    let normalized = message
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.is_empty() {
        return None;
    }

    let mut sanitized = normalized
        .chars()
        .take(MAX_HEALTH_MESSAGE_CHARS)
        .collect::<String>();
    if normalized.chars().count() > MAX_HEALTH_MESSAGE_CHARS {
        sanitized.push('…');
    }
    Some(sanitized)
}

pub fn apply_health_observation(
    account: &mut StoredAccount,
    observation: AccountHealthObservation,
) {
    let now = Utc::now();
    let is_same_signal = account.health.as_ref().is_some_and(|current| {
        current.status == observation.status && current.error_code == observation.error_code
    });

    if is_same_signal {
        if let Some(current) = account.health.as_mut() {
            current.source = observation.source;
            current.http_status = observation.http_status;
            current.last_seen_at = now;
            current.occurrence_count = current.occurrence_count.saturating_add(1);
            if observation.message.is_some() {
                current.message = observation.message;
            }
        }
    } else {
        if let Some(previous) = account.health.take() {
            account.health_history.push(previous);
        }
        account.health = Some(AccountHealthDiagnostic {
            status: observation.status,
            source: observation.source,
            http_status: observation.http_status,
            error_code: observation.error_code,
            message: observation.message,
            first_seen_at: now,
            last_seen_at: now,
            occurrence_count: 1,
        });
    }

    let cutoff = now - Duration::days(HEALTH_HISTORY_RETENTION_DAYS);
    account
        .health_history
        .retain(|diagnostic| diagnostic.last_seen_at >= cutoff);
    if account.health_history.len() > MAX_HEALTH_HISTORY {
        let overflow = account.health_history.len() - MAX_HEALTH_HISTORY;
        account.health_history.drain(..overflow);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_invalidated_requires_reauthentication() {
        let observation = classify_http_error(
            AccountHealthSource::Usage,
            401,
            r#"{"error":{"code":"token_invalidated","message":"Please sign in again."}}"#,
        );
        assert_eq!(observation.status, AccountHealthStatus::ReauthRequired);
        assert_eq!(observation.error_code.as_deref(), Some("token_invalidated"));
    }

    #[test]
    fn exact_workspace_deactivation_is_not_treated_as_generic_auth_failure() {
        let observation = classify_http_error(
            AccountHealthSource::Usage,
            402,
            r#"{"error":{"code":"deactivated_workspace","message":"Workspace unavailable"}}"#,
        );
        assert_eq!(
            observation.status,
            AccountHealthStatus::WorkspaceDeactivated
        );
    }

    #[test]
    fn generic_forbidden_is_transient_instead_of_banned() {
        let observation = classify_http_error(AccountHealthSource::AccountsCheck, 403, "Forbidden");
        assert_eq!(observation.status, AccountHealthStatus::TransientError);
    }

    #[test]
    fn a_generic_deactivated_word_does_not_mark_an_account_deactivated() {
        let observation = classify_http_error(
            AccountHealthSource::Usage,
            400,
            "A deprecated feature was deactivated",
        );
        assert_eq!(observation.status, AccountHealthStatus::Unknown);
    }

    #[test]
    fn repeated_signal_updates_current_diagnostic_without_growing_history() {
        let mut account = StoredAccount::new_api_key("test".into(), "key".into());
        let observation = classify_http_error(
            AccountHealthSource::Usage,
            401,
            r#"{"error":{"code":"token_invalidated","message":"Sign in again"}}"#,
        );
        apply_health_observation(&mut account, observation.clone());
        apply_health_observation(&mut account, observation);

        assert_eq!(account.health_history.len(), 0);
        assert_eq!(
            account
                .health
                .as_ref()
                .map(|diagnostic| diagnostic.occurrence_count),
            Some(2)
        );

        apply_health_observation(
            &mut account,
            healthy_observation(AccountHealthSource::Usage),
        );
        assert_eq!(account.health_history.len(), 1);
        assert_eq!(
            account.health.as_ref().map(|diagnostic| diagnostic.status),
            Some(AccountHealthStatus::Healthy)
        );
    }
}
