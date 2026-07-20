// Types matching the Rust backend

export type AuthMode = "api_key" | "chat_g_p_t";
export type DockDisplayMode = "show_in_dock" | "menu_bar_only";
export type TaskbarLayout = "detailed" | "minimal" | "compact";
export type TaskbarDoubleClickAction = "toggle_floating" | "open_main";
export type FloatingField = "account" | "primary_usage" | "primary_reset" | "secondary_usage";

export interface AppSettings {
  tray_display_mode: "icon_and_session" | "active_usage_text" | "hidden";
  dock_display_mode: DockDisplayMode;
  language: string;
  close_behavior_prompt_enabled: boolean;
  taskbar: {
    enabled: boolean;
    layout: TaskbarLayout;
    double_click_action: TaskbarDoubleClickAction;
    last_error: string | null;
    offset_x: number;
    offset_y: number;
  };
  floating: {
    enabled: boolean;
    visible: boolean;
    click_through: boolean;
    always_on_top: boolean;
    compact_mode: boolean;
    opacity: number;
    position: [number, number] | null;
    size: [number, number] | null;
    visible_fields: FloatingField[];
  };
}

export interface AccountInfo {
  id: string;
  name: string;
  disabled: boolean;
  email: string | null;
  plan_type: string | null;
  subscription_expires_at: string | null;
  auth_mode: AuthMode;
  is_active: boolean;
  created_at: string;
  last_used_at: string | null;
  has_codex_config: boolean;
}

export interface UsageInfo {
  account_id: string;
  plan_type: string | null;
  primary_used_percent: number | null;
  primary_window_minutes: number | null;
  primary_resets_at: number | null;
  secondary_used_percent: number | null;
  secondary_window_minutes: number | null;
  secondary_resets_at: number | null;
  has_credits: boolean | null;
  unlimited_credits: boolean | null;
  credits_balance: string | null;
  error: string | null;
}

export interface AccountUsageSummary {
  lifetime_tokens: number | null;
  peak_daily_tokens: number | null;
  longest_task_seconds: number | null;
  current_streak_days: number | null;
  longest_streak_days: number | null;
}

export interface AccountUsageActivity {
  fast_mode_percent: number | null;
  reasoning_effort: string | null;
  reasoning_effort_percent: number | null;
  skills_explored: number | null;
  total_skills_used: number | null;
  total_threads: number | null;
}

export interface AccountDailyUsage {
  date: string;
  tokens: number;
}

export interface AccountTopInvocation {
  kind: string;
  display_name: string;
  usage_count: number;
  plugin_id: string | null;
  plugin_name: string | null;
  skill_id: string | null;
  skill_name: string | null;
}

export interface AccountResetCredit {
  id: string;
  reset_type: string;
  status: string;
  granted_at: string | null;
  expires_at: string | null;
  redeem_started_at: string | null;
  redeemed_at: string | null;
  title: string | null;
  description: string | null;
}

export interface AccountResetCredits {
  available_count: number;
  next_expires_at: string | null;
  credits: AccountResetCredit[];
}

export const ACCOUNT_USAGE_SOURCE_CHATGPT_BACKEND = "chatgpt_backend";

export interface AccountUsageStats {
  account_id: string;
  available: boolean;
  source: string;
  generated_at: string | null;
  stats_as_of: string | null;
  summary: AccountUsageSummary;
  activity: AccountUsageActivity;
  daily: AccountDailyUsage[];
  top_invocations: AccountTopInvocation[];
  reset_credits: AccountResetCredits | null;
  error: string | null;
}

export interface OAuthLoginInfo {
  auth_url: string;
  callback_port: number;
}

export interface AccountWithUsage extends AccountInfo {
  usage?: UsageInfo;
  usageLoading?: boolean;
  usageUpdatedAt?: number;
}

export interface CodexProcessInfo {
  count: number;
  background_count: number;
  can_switch: boolean;
  pids: number[];
}

export interface WarmupSummary {
  total_accounts: number;
  warmed_accounts: number;
  failed_account_ids: string[];
  failed_accounts?: WarmupFailure[];
}

export interface WarmupFailure {
  account_id: string;
  error: string;
}

export interface WarmupFailureInfo {
  error: string;
  failedAt: number;
  modelUnavailable: boolean;
}

export interface ImportAccountsSummary {
  total_in_payload: number;
  imported_count: number;
  skipped_count: number;
}
