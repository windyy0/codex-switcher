# Changelog

This document records the user-facing changes in Codex Switcher. Work in progress is kept in the “Unreleased” section and moved into the matching version section when a release is prepared.

The Chinese version is maintained in [CHANGELOG.md](./CHANGELOG.md); both files must contain matching release versions.

## [Unreleased]

### Added

- Added a “Record deactivation email” action to ChatGPT account details. Users can paste an OpenAI deactivation notice without starting reauthorization; Codex Switcher verifies the account email and persists the notice date as structured diagnostic data.

### Improved

- Reauthorization now matches the add-account login flow: after generating a login link, users can choose “Copy” or “Open” instead of the browser opening automatically, with explicit feedback when copying fails and clear email-recognition results.
- The Codex process indicator in the upper-left is now gray when no process is running and green when processes are detected; the “Open Codex” button is now blue.
- Disabled accounts continue to show explicit deactivation diagnostics, email source, and deactivation date so the recorded information can be reviewed or updated later.

## [0.107.0] - 2026-07-29

### Added

- Added account health diagnostics that distinguish expired authorization, account deactivation, workspace deactivation, request limits, and transient failures from explicit usage, account-check, token-refresh, and login responses, while retaining sanitized error codes, provider messages, timestamps, and recent diagnostic history.
- ChatGPT accounts with expired authorization can now be reauthorized in place; successful authorization updates the existing credentials after identity verification, without creating a duplicate account or changing the active account when the target is inactive.
- When the authorization page does not return to Codex Switcher, users can confirm the common deleted-or-deactivated message with one click or paste the full page error for strict recognition; reporting immediately stops the wait and records that the signal was confirmed from the authorization page.

### Improved

- Added direct “Reauthorize,” “Recheck status,” and “View diagnostics” entry points to account rows, and aligned expired-authorization and manually disabled statuses in the same list column.
- Accounts with expired authorization or an explicit deactivation signal are excluded from automatic switching, bulk usage polling, and automatic warm-up; manual status checks can still use the existing credentials to detect recovery.
- Account deletion now uses a confirmation dialog and clearly reports success, failure, and the failure reason.

### Fixed

- Fixed ChatGPT sign-in being unable to update an existing Codex Switcher account after its credentials expired, while importing the same account again only reported that it already existed.
- Fixed Codex Switcher having no way to stop waiting or record the real deactivation signal when OpenAI keeps `account_deactivated` and similar authentication errors on a provider page without an OAuth callback.

## [0.106.5] - 2026-07-29

### Added

- Reset-credit badges can now expand to show each available credit, its name, and its expiry time in local time, sorted by the nearest expiry.

### Improved

- Improved Windows taskbar account-name rendering while keeping the “Account:” prefix; long names now use smart middle ellipsis, show the full name on hover, and reserve safe spacing to prevent overlap.

### Fixed

- ChatGPT usage, account-check, and warm-up requests now use browser-like request headers to reduce Cloudflare 403 blocks.

## [0.106.4] - 2026-07-26

### Fixed

- Fixed Windows in-app updates falling back to MSI and recreating a deleted desktop shortcut; updates now consistently use the NSIS installer.
- Fixed the “Minimize” tooltip appearing after restoring the main window from the taskbar when the pointer is not over the button.

## [0.106.3] - 2026-07-26

### Improved

- Free account quotas now use the 30-day window returned by the API and display as “Monthly Limit”; Plus accounts continue to display “Weekly Limit”.
- Unified quota-period labels across the main page, floating widget, Windows taskbar, and tray; Free accounts no longer show a misleading weekly quota when none is returned.
- Expiry and reset-credit timestamps now show minute precision when they are in the red warning state, while other states retain the existing date-only display.
- Updated the GitHub Actions checkout, setup-node, and pnpm versions, and migrated the Node version used by workflow project scripts and builds to Node 24.

### Fixed

- Fixed expired subscriptions continuing to display Plus and leaving stale quota values in the Windows taskbar.

## [0.106.2] - 2026-07-26

### Fixed

- Fixed a startup race that could leave the language selector on “System Default” after the saved language was loaded asynchronously.

## [0.106.1] - 2026-07-26

### Added

- Added an optional dual-zone clock that shows local time and OpenAI/UTC time, date, and weekday in the title bar.

### Improved

- Centered the dual-zone clock in the title bar, limited the display to minutes, and collapsed it on narrow windows.

### Fixed

- Fixed Windows installs and silent updates recreating a desktop shortcut after the user removed it.

## [0.106.0] - 2026-07-23

### Added

- Update prompts now show the highlights for the new version and link to the complete release history.
- Release notes now support Chinese and English; English environments show English highlights and open the English changelog.

### Improved

- Unified the app and taskbar tray icons; clicking the tray icon now opens the main window directly.
- Reworked the tray context menu to group the current account, quick actions, and display components.
- Switched to native Windows process detection and track only actual Codex desktop processes, reducing blocking checks before switching.
- Strengthened the release tooling with strict version, bilingual changelog, signature, and cross-platform updater validation.

### Fixed

- Fixed account switching hanging when Windows process detection accumulated stale work.
- Fixed multiple checkmarks appearing in the tray menu after cancelling an account switch.
- Fixed inconsistent or duplicated state when multiple tray account switch requests were sent in quick succession.
- Fixed stale process status remaining after a failed check and missing feedback when refreshing one account failed.

## [0.105.0] - 2026-07-23

### Added

- Added list and card layouts for the main account page.
- Added persistence for the main window size, position, and maximized state.

### Improved

- Reorganized the title bar, account actions, and layout toggle.
- Merged account details into the card layout to reduce extra navigation.
- Improved account filters and wording for subscription expiry and API accounts.
