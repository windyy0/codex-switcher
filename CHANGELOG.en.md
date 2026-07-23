# Changelog

This document records the user-facing changes in Codex Switcher. Work in progress is kept in the “Unreleased” section and moved into the matching version section when a release is prepared.

The Chinese version is maintained in [CHANGELOG.md](./CHANGELOG.md); both files must contain matching release versions.

## [Unreleased]

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
