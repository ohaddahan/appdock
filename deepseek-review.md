# AppDock Code Review

**Project:** Native macOS workspace manager for existing application windows  
**Language:** Rust (2024 edition)  
**Lines:** ~5,264 across 12 source files  
**Tests:** 39 unit tests + 17 CLI smoke/fixture modes  

## Verdict

No bugs found. The codebase is production-quality with strong architecture, thread safety, rollback patterns, and generation-counter stale-command detection throughout.

## Recommendations

### 1. Timer-based intervals instead of tick-count magic numbers (src/ui.rs:838-857)

`count.is_multiple_of(14)` / `count.is_multiple_of(70)` ties app-list publishing and discovery to a 150 ms timer tick. If the tick interval ever changes, these ratios break silently.

```rust
// Current — fragile:
if count.is_multiple_of(14) { /* ~2.1s */ }
if count.is_multiple_of(70) { /* ~10.5s */ }

// Better — use std::time::Instant:
const APP_PUBLISH_INTERVAL: Duration = Duration::from_millis(2100);
const DISCOVER_INTERVAL: Duration = Duration::from_millis(10500);
```

### 2. Single-window minimize shouldn't pause the whole engine (src/engine.rs:321-328)

When *any* docked window is minimized by the user, `observe()` sets `self.paused`, stopping resize/follow for **all** docked windows. Consider per-window granularity instead of a global pause.

### 3. `set_frame()` settle loop returns before stability (src/macos.rs:352-363)

The read-back loop breaks on the first iteration where `next.near(actual)`, but the frame may still be settling from native window manager constraints. Consider a brief settle wait after the loop exits.

### 4. Redundant `focus_suspended` checks in `focus()` (src/macos.rs:397-435)

Lines 398, 403, 409, and 414 all read `focus_suspended` mid-operation. The `EditingBarrier` already prevents `Switch`/`Raise` from reaching the worker, making these mid-operation checks in `focus()` itself redundant with the pre-command check in `src/worker.rs:221`.

### 5. Encapsulate `MacBackend::apps` (src/macos.rs:29)

`apps: Vec<App>` is public and set directly via `Command::Apps` (`src/worker.rs:239`). An `update_apps()` method would encapsulate any future validation.

### 6. Consider `Drop` for `Backdrop` (src/backdrop.rs:17)

If the `Backdrop` is dropped without calling `hide()`, the borderless window remains on screen. A `Drop` impl that calls `orderOut(None)` would prevent orphan windows.