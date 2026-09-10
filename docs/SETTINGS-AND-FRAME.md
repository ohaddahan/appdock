# Visible settings, save/reset, and corner appearance — 2026-09-10

## Controls

- **Settings** is visible beside **Add App** in the title bar. **AppDock → Settings…** and **Command-comma** are alternate entry points while AppDock is active. The older **Auto-add at Startup** submenu remains compatible.
- Checking/unchecking individual apps saves automatically.
- **Save Current Apps for Startup** replaces startup choices with the currently attached apps, once per bundle, in tab order.
- **Reset Saved App Choices** clears the startup choices. It does not release current tabs, discard recovery snapshots, or reset geometry/shortcuts.
- Both actions are persisted through the worker into the existing workspace JSON. Settings changes cancel pending startup work. The popup waits for the focus barrier, and later picker/rename/quit intents cancel a pending popup.

## Corner change

The supplied 36×37 crop appears to show a second edge/shadow at a rounded native app corner. AppDock's controls window previously retained its own shadow despite its rectangular transparent cutout, and its backdrop color differed from the surrounding frame. The controls shadow is now disabled while docked, and the backdrop uses the frame color. The external app keeps its native border, rounded corners, and shadow.

This is a scoped correction to AppDock's own framing. The small crop was not reproduced as an identical before/after desktop capture; the exact reported triangle still needs a visual check in the rebuilt app. No existing user application window was changed for validation.

## Validation

- **72 deterministic tests pass**, including `save_and_reset_startup_choices_do_not_change_live_windows_or_other_preferences`.
- Formatting, strict Clippy, locked build, Rust 1.95 all-target/all-feature checks, and whitespace checks pass. Logs: [tests](review-evidence/settings-surface/tests.log), [Clippy](review-evidence/settings-surface/clippy.log), [build](review-evidence/settings-surface/build.log), [Rust 1.95](review-evidence/settings-surface/msrv.log).
- [Targeted native manifest](review-evidence/settings-surface/results.json): **settings, surface, and Startup all pass**. The Settings test presses the actual title-bar button and verifies native menu tracking with Save/Reset actions. Surface testing verifies no controls-window shadow, transparent app area, and opaque surrounding frame. Startup testing saves/resets/reads back preferences while verifying the attached window stays unchanged.
- [Broader native run](review-evidence/settings-corners/results.json): Startup, A2 rendering, backdrop lifetime, and design pass. Frame failed before reaching the new Settings step on native target focus acquisition; pointer testing later found another window covering the sampled controls point. Both failures are retained. Full frame/pointer behavior in this desktop session is not claimed verified.
- Fixtures use disposable windows and isolated data directories. No installed bundle, production workspace, or user app was modified. The pre-existing Dock-icon timing edit in `src/ui.rs` was preserved.

Run focused checks with:

```sh
cargo build --all-features --locked
python3 scripts/review-native.py --output /tmp/appdock-settings-review --cases settings,surface,Startup
```
