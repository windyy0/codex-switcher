//! Core types for Codex Switcher

use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The main storage structure for all accounts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountsStore {
    /// Schema version for future migrations
    pub version: u32,
    /// List of all stored accounts
    pub accounts: Vec<StoredAccount>,
    /// Currently active account ID
    pub active_account_id: Option<String>,
    /// Canonical CODEX_HOME whose derived auth/config files correspond to the
    /// global active account. Missing means a legacy store not yet migrated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_account_home: Option<String>,
    /// Set of account IDs that are masked (hidden)
    #[serde(default)]
    pub masked_account_ids: Vec<String>,
    /// Whether the user's normal config.toml has been captured before entering
    /// an API account with a per-account configuration.
    #[serde(default)]
    pub codex_config_backup_captured: bool,
    /// Legacy inline backup retained only to migrate development builds that
    /// stored the original config in accounts.json. New backups live in
    /// ~/.codex-switcher/config.toml.backup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_config_backup: Option<String>,
    #[serde(default)]
    pub codex_config_backup_existed: bool,
    /// Canonical CODEX_HOME path associated with the active backup session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_config_backup_home: Option<String>,
    /// Exact API overlay currently applied to config.toml. This lets switching
    /// remove only managed keys while preserving unrelated changes made by Codex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_config_active_overlay: Option<String>,
    /// Crash-recovery journal for the cross-file config.toml/store commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_config_transition: Option<CodexConfigTransitionJournal>,
    /// Active credentials were durably rotated in the account store but the
    /// derived auth.json may still need to be rebuilt after a crash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_auth_sync_account_id: Option<String>,
    /// Canonical CODEX_HOME that must receive the pending auth.json update.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_auth_sync_home: Option<String>,
    /// Digests of refresh tokens that were already exchanged. Keeping a small
    /// durable history prevents another process from consuming an old rotating
    /// token after it waited for credentials.lock.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consumed_refresh_token_hashes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexConfigTransitionJournal {
    pub home: String,
    /// Account that the complete auth/config transition intends to activate.
    /// None means remove managed credentials after deleting the last account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_account_id: Option<String>,
    /// Complete target record and optional deletion are part of the same
    /// durable transaction as auth/config/active-account changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_account: Option<StoredAccount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove_account_id: Option<String>,
    pub before_config_hash: Option<String>,
    pub after_config_hash: Option<String>,
    pub before: CodexConfigManagedState,
    pub after: CodexConfigManagedState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexConfigManagedState {
    pub backup_captured: bool,
    pub backup_existed: bool,
    pub backup_home: Option<String>,
    pub active_overlay: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayDisplayMode {
    IconAndSession,
    #[default]
    ActiveUsageText,
    Hidden,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockDisplayMode {
    #[default]
    ShowInDock,
    MenuBarOnly,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskbarLayout {
    #[default]
    Detailed,
    Minimal,
    Compact,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskbarDoubleClickAction {
    #[default]
    ToggleFloating,
    OpenMain,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskbarSettings {
    pub enabled: bool,
    pub layout: TaskbarLayout,
    pub double_click_action: TaskbarDoubleClickAction,
    pub last_error: Option<String>,
    pub offset_x: i32,
    pub offset_y: i32,
}

impl Default for TaskbarSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            layout: TaskbarLayout::Detailed,
            double_click_action: TaskbarDoubleClickAction::ToggleFloating,
            last_error: None,
            offset_x: 0,
            offset_y: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FloatingField {
    Account,
    PrimaryUsage,
    PrimaryReset,
    SecondaryUsage,
}

fn default_floating_fields() -> Vec<FloatingField> {
    vec![
        FloatingField::Account,
        FloatingField::PrimaryUsage,
        FloatingField::PrimaryReset,
        FloatingField::SecondaryUsage,
    ]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FloatingSettings {
    pub enabled: bool,
    pub visible: bool,
    pub click_through: bool,
    pub always_on_top: bool,
    pub compact_mode: bool,
    pub opacity: f64,
    pub position: Option<(i32, i32)>,
    pub size: Option<(u32, u32)>,
    pub visible_fields: Vec<FloatingField>,
}

impl FloatingSettings {
    pub fn normalize_modes(&mut self, previous: Option<&Self>) -> bool {
        if !self.compact_mode || !self.click_through {
            return false;
        }

        let click_through_was_the_only_new_mode = previous.is_some_and(|old| {
            old.click_through != self.click_through && old.compact_mode == self.compact_mode
        });
        if click_through_was_the_only_new_mode {
            self.compact_mode = false;
        } else {
            self.click_through = false;
        }
        true
    }
}

impl Default for FloatingSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            visible: false,
            click_through: false,
            always_on_top: true,
            compact_mode: false,
            opacity: 0.92,
            position: None,
            size: None,
            visible_fields: default_floating_fields(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AppLanguage(String);

impl AppLanguage {
    pub const DEFAULT_CODE: &'static str = "en-US";
    pub const SYSTEM_CODE: &'static str = "system";

    pub fn new(code: impl Into<String>) -> Self {
        Self(code.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for AppLanguage {
    fn default() -> Self {
        Self::new(Self::SYSTEM_CODE)
    }
}

fn default_close_behavior_prompt_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub tray_display_mode: TrayDisplayMode,
    pub dock_display_mode: DockDisplayMode,
    pub language: AppLanguage,
    #[serde(default = "default_close_behavior_prompt_enabled")]
    pub close_behavior_prompt_enabled: bool,
    pub taskbar: TaskbarSettings,
    pub floating: FloatingSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            tray_display_mode: TrayDisplayMode::default(),
            dock_display_mode: DockDisplayMode::default(),
            language: AppLanguage::default(),
            close_behavior_prompt_enabled: true,
            taskbar: TaskbarSettings::default(),
            floating: FloatingSettings::default(),
        }
    }
}

impl Default for AccountsStore {
    fn default() -> Self {
        Self {
            version: 1,
            accounts: Vec::new(),
            active_account_id: None,
            active_account_home: None,
            masked_account_ids: Vec::new(),
            codex_config_backup_captured: false,
            codex_config_backup: None,
            codex_config_backup_existed: false,
            codex_config_backup_home: None,
            codex_config_active_overlay: None,
            codex_config_transition: None,
            pending_auth_sync_account_id: None,
            pending_auth_sync_home: None,
            consumed_refresh_token_hashes: Vec::new(),
        }
    }
}

/// A stored account with all its metadata and credentials
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredAccount {
    /// Unique identifier (UUID)
    pub id: String,
    /// User-defined display name
    pub name: String,
    /// Whether automatic requests and account switching are disabled.
    #[serde(default)]
    pub disabled: bool,
    /// Email extracted from ID token (for ChatGPT auth)
    pub email: Option<String>,
    /// Plan type: free, plus, pro, team, business, enterprise, edu
    pub plan_type: Option<String>,
    /// Subscription expiration extracted from ChatGPT ID token, when available
    #[serde(default)]
    pub subscription_expires_at: Option<DateTime<Utc>>,
    /// Authentication mode
    pub auth_mode: AuthMode,
    /// Authentication credentials
    pub auth_data: AuthData,
    /// Optional per-account Codex configuration.  API providers commonly need
    /// their own base URL, model, or wire protocol in addition to the key.
    #[serde(default)]
    pub codex_config: Option<String>,
    /// When the account was added
    pub created_at: DateTime<Utc>,
    /// Last time this account was used
    pub last_used_at: Option<DateTime<Utc>>,
}

impl StoredAccount {
    /// Create a new account with API key authentication
    pub fn new_api_key(name: String, api_key: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            disabled: false,
            email: None,
            plan_type: None,
            subscription_expires_at: None,
            auth_mode: AuthMode::ApiKey,
            auth_data: AuthData::ApiKey { key: api_key },
            codex_config: None,
            created_at: Utc::now(),
            last_used_at: None,
        }
    }

    /// Create a new account with ChatGPT OAuth authentication
    pub fn new_chatgpt(
        name: String,
        email: Option<String>,
        plan_type: Option<String>,
        subscription_expires_at: Option<DateTime<Utc>>,
        id_token: String,
        access_token: String,
        refresh_token: String,
        account_id: Option<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            disabled: false,
            email,
            plan_type,
            subscription_expires_at,
            auth_mode: AuthMode::ChatGPT,
            auth_data: AuthData::ChatGPT {
                id_token,
                access_token,
                refresh_token,
                account_id,
            },
            codex_config: None,
            created_at: Utc::now(),
            last_used_at: None,
        }
    }
}

/// Authentication mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// Using an OpenAI API key
    ApiKey,
    /// Using ChatGPT OAuth tokens
    ChatGPT,
}

/// Authentication data (credentials)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthData {
    /// API key authentication
    ApiKey {
        /// The API key
        key: String,
    },
    /// ChatGPT OAuth authentication
    ChatGPT {
        /// JWT ID token containing user info
        id_token: String,
        /// Access token for API calls
        access_token: String,
        /// Refresh token for token renewal
        refresh_token: String,
        /// ChatGPT account ID
        account_id: Option<String>,
    },
}

#[derive(Debug, Clone, Default)]
pub struct ChatGptIdTokenClaims {
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub account_id: Option<String>,
    pub subscription_expires_at: Option<DateTime<Utc>>,
}

pub fn parse_chatgpt_id_token_claims(id_token: &str) -> ChatGptIdTokenClaims {
    let parts: Vec<&str> = id_token.split('.').collect();
    if parts.len() != 3 {
        return ChatGptIdTokenClaims::default();
    }

    let payload = match base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[1]) {
        Ok(bytes) => bytes,
        Err(_) => return ChatGptIdTokenClaims::default(),
    };

    let json: serde_json::Value = match serde_json::from_slice(&payload) {
        Ok(value) => value,
        Err(_) => return ChatGptIdTokenClaims::default(),
    };

    let auth_claims = json.get("https://api.openai.com/auth");

    ChatGptIdTokenClaims {
        email: json.get("email").and_then(|v| v.as_str()).map(String::from),
        plan_type: auth_claims
            .and_then(|auth| auth.get("chatgpt_plan_type"))
            .and_then(|v| v.as_str())
            .map(String::from),
        account_id: auth_claims
            .and_then(|auth| auth.get("chatgpt_account_id"))
            .and_then(|v| v.as_str())
            .map(String::from),
        subscription_expires_at: auth_claims
            .and_then(|auth| auth.get("chatgpt_subscription_active_until"))
            .and_then(|v| v.as_str())
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc)),
    }
}

// ============================================================================
// Types for Codex's auth.json format (for compatibility)
// ============================================================================

/// The official Codex auth.json format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthDotJson {
    /// OpenAI API key (for API key auth mode)
    #[serde(rename = "OPENAI_API_KEY", skip_serializing_if = "Option::is_none")]
    pub openai_api_key: Option<String>,
    /// OAuth tokens (for ChatGPT auth mode)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TokenData>,
    /// Last token refresh timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_refresh: Option<DateTime<Utc>>,
}

/// Token data stored in auth.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenData {
    /// JWT ID token
    pub id_token: String,
    /// Access token
    pub access_token: String,
    /// Refresh token
    pub refresh_token: String,
    /// Account ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

// ============================================================================
// Types for frontend communication
// ============================================================================

/// Account info sent to the frontend (without sensitive data)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountInfo {
    pub id: String,
    pub name: String,
    pub disabled: bool,
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub subscription_expires_at: Option<DateTime<Utc>>,
    pub auth_mode: AuthMode,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub has_codex_config: bool,
}

impl AccountInfo {
    pub fn from_stored(account: &StoredAccount, active_id: Option<&str>) -> Self {
        let fallback_subscription_expires_at = match &account.auth_data {
            AuthData::ChatGPT { id_token, .. } => {
                parse_chatgpt_id_token_claims(id_token).subscription_expires_at
            }
            AuthData::ApiKey { .. } => None,
        };

        Self {
            id: account.id.clone(),
            name: account.name.clone(),
            disabled: account.disabled,
            email: account.email.clone(),
            plan_type: account.plan_type.clone(),
            subscription_expires_at: account
                .subscription_expires_at
                .clone()
                .or(fallback_subscription_expires_at),
            auth_mode: account.auth_mode,
            is_active: active_id == Some(&account.id),
            created_at: account.created_at,
            last_used_at: account.last_used_at,
            has_codex_config: account.codex_config.is_some(),
        }
    }
}

/// Usage information for an account
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageInfo {
    /// Account ID
    pub account_id: String,
    /// Plan type
    pub plan_type: Option<String>,
    /// Primary rate limit window usage (percentage 0-100)
    pub primary_used_percent: Option<f64>,
    /// Primary window duration in minutes
    pub primary_window_minutes: Option<i64>,
    /// Primary window reset timestamp (unix seconds)
    pub primary_resets_at: Option<i64>,
    /// Secondary rate limit window usage (percentage 0-100)
    pub secondary_used_percent: Option<f64>,
    /// Secondary window duration in minutes
    pub secondary_window_minutes: Option<i64>,
    /// Secondary window reset timestamp (unix seconds)
    pub secondary_resets_at: Option<i64>,
    /// Whether the account has credits
    pub has_credits: Option<bool>,
    /// Whether credits are unlimited
    pub unlimited_credits: Option<bool>,
    /// Credit balance string (e.g., "$10.50")
    pub credits_balance: Option<String>,
    /// Error message if usage fetch failed
    pub error: Option<String>,
}

impl UsageInfo {
    pub fn error(account_id: String, error: String) -> Self {
        Self {
            account_id,
            plan_type: None,
            primary_used_percent: None,
            primary_window_minutes: None,
            primary_resets_at: None,
            secondary_used_percent: None,
            secondary_window_minutes: None,
            secondary_resets_at: None,
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: Some(error),
        }
    }
}

/// Warm-up execution summary across accounts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarmupSummary {
    /// Number of accounts that were targeted
    pub total_accounts: usize,
    /// Number of accounts whose warm-up request succeeded
    pub warmed_accounts: usize,
    /// Account IDs whose warm-up request failed
    pub failed_account_ids: Vec<String>,
    /// Failure details for displaying and grouping actionable errors
    #[serde(default)]
    pub failed_accounts: Vec<WarmupFailure>,
}

/// A failed warm-up request and its backend error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarmupFailure {
    pub account_id: String,
    pub error: String,
}

/// Import summary for account config import operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportAccountsSummary {
    /// Number of accounts found in the imported payload.
    pub total_in_payload: usize,
    /// Number of accounts actually imported.
    pub imported_count: usize,
    /// Number of accounts skipped because they already exist.
    pub skipped_count: usize,
}

/// OAuth login information returned to frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthLoginInfo {
    /// The authorization URL to open in browser
    pub auth_url: String,
    /// The local callback port
    pub callback_port: u16,
}

// ============================================================================
// API Response types (from Codex backend)
// ============================================================================

/// Rate limit status from API
#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitStatusPayload {
    pub plan_type: String,
    #[serde(default)]
    pub rate_limit: Option<RateLimitDetails>,
    #[serde(default)]
    pub credits: Option<CreditStatusDetails>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitDetails {
    pub primary_window: Option<RateLimitWindow>,
    pub secondary_window: Option<RateLimitWindow>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitWindow {
    pub used_percent: f64,
    pub limit_window_seconds: Option<i32>,
    pub reset_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreditStatusDetails {
    pub has_credits: bool,
    pub unlimited: bool,
    #[serde(default)]
    pub balance: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{
        parse_chatgpt_id_token_claims, AppLanguage, AppSettings, DockDisplayMode, FloatingSettings,
        StoredAccount, TrayDisplayMode,
    };
    use base64::Engine;

    #[test]
    fn parses_subscription_expiry_from_realistic_id_token_claims() {
        let payload = r#"{"email":"user@example.com","https://api.openai.com/auth":{"chatgpt_plan_type":"plus","chatgpt_account_id":"acc_123","chatgpt_subscription_active_until":"2026-04-23T05:03:38+00:00"}}"#;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload);
        let token = format!("header.{encoded}.signature");

        let claims = parse_chatgpt_id_token_claims(&token);

        assert_eq!(claims.email.as_deref(), Some("user@example.com"));
        assert_eq!(claims.plan_type.as_deref(), Some("plus"));
        assert_eq!(claims.account_id.as_deref(), Some("acc_123"));
        assert_eq!(
            claims
                .subscription_expires_at
                .map(|value| value.to_rfc3339()),
            Some("2026-04-23T05:03:38+00:00".to_string())
        );
    }

    #[test]
    fn legacy_accounts_default_to_enabled() {
        let account = StoredAccount::new_api_key("Legacy".into(), "sk-test".into());
        let mut value = serde_json::to_value(account).unwrap();
        value.as_object_mut().unwrap().remove("disabled");

        let decoded: StoredAccount = serde_json::from_value(value).unwrap();

        assert!(!decoded.disabled);
    }

    #[test]
    fn app_settings_default_missing_dock_display_mode_to_show_in_dock() {
        let settings: AppSettings =
            serde_json::from_str(r#"{"tray_display_mode":"active_usage_text"}"#).unwrap();

        assert_eq!(settings.tray_display_mode, TrayDisplayMode::ActiveUsageText);
        assert_eq!(settings.dock_display_mode, DockDisplayMode::ShowInDock);
        assert_eq!(settings.language, AppLanguage::default());
        assert!(settings.close_behavior_prompt_enabled);
        assert!(settings.taskbar.enabled);
        assert_eq!(settings.taskbar.offset_x, 0);
        assert_eq!(settings.taskbar.offset_y, 0);
        assert!(settings.floating.enabled);
        assert!(!settings.floating.visible);
        assert!(!settings.floating.compact_mode);
        assert_eq!(settings.floating.opacity, 0.92);
    }

    #[test]
    fn floating_compact_and_click_through_modes_are_mutually_exclusive() {
        let mut legacy = FloatingSettings {
            compact_mode: true,
            click_through: true,
            ..FloatingSettings::default()
        };
        assert!(legacy.normalize_modes(None));
        assert!(legacy.compact_mode);
        assert!(!legacy.click_through);

        let previous = legacy.clone();
        let mut enable_click_through = previous.clone();
        enable_click_through.click_through = true;
        assert!(enable_click_through.normalize_modes(Some(&previous)));
        assert!(!enable_click_through.compact_mode);
        assert!(enable_click_through.click_through);
    }
}
