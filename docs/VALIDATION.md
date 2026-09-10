# Validation — 2026-09-10

Host: Apple Silicon (`arm64`), macOS 26.6.2 (25G83), Rust 1.97.1. Installed versions read directly from application bundle plists: **Discord 0.0.410** (`com.hnc.Discord`) and **Telegram Lite 7.1.4** (`org.telegram.desktop`).

## Automated checks

- `cargo fmt --all --check`: passed.
- `cargo clippy --all-targets --all-features --locked -- -D warnings`: passed.
- `cargo test --all-features --locked`: 19 tests passed.
- Locked local build: passed.
- Local `.app` packaging, plist validation, and strict ad-hoc signature verification: passed. No notarization or distribution.

Fake-backend regressions cover exact-handle switching with duplicate titles, rollback on focus/minimization failure, stale switch cancellation, release/exit restoration, failed-release retry, target closure, manual movement, permission loss, fullscreen rejection, and ambiguous saved identities. Persistence checks cover labels/order and preserving unsupported versions. Geometry checks include displays above/left of the primary display.

## Native evidence

The initial sandbox diagnostic reported `Accessibility trusted: false`. The outside-sandbox diagnostic reported `true` and found exactly one eligible standard window in each target app. This distinguishes sandbox restrictions from the desktop process's actual access; it does not establish permission for every launch context.

The first native prototype run exposed AX timeout `-25204`, including a failed restoration attempt. A later run exposed a minimization readback race. Changes: one-second per-request AX timeout, idempotent minimization, and bounded polling through macOS animation completion. Subsequent native runs succeeded and restoration was read back.

**Backend smoke:** six complete Discord/Telegram switching cycles passed, including exact focused-window verification, selected-window minimization state, and target movement/resizing. Restoration readback returned `Ok(())`.

**Integrated AppKit smoke:** twelve tab-switch requests completed through the native manager and asynchronous worker, with selection acknowledgments before subsequent stages. The manager moved/resized, the first tab released successfully, and exit restored the remaining window. The process exited normally. Final diagnostic readback matched the pre-test snapshots:

| App | Original and final AX frame (x, y, width, height) | Original and final minimized state |
| --- | --- | --- |
| Discord | `(76, 160, 1233, 1091)` | `true` |
| Telegram Lite | `(49, 254, 1215, 884)` | `false` |

No messages were typed or sent, accounts changed, or target applications launched/quit. Native smoke actions were restricted to existing window geometry, focus and minimization.

**Native startup/quit:** the isolated AppKit UI fixture rendered and exited normally. [Workspace screenshot](screenshots/native-workspace.png) was captured through macOS window capture and inspected. It shows the native manager only. AppKit's internal bitmap capture omitted composited control details and is not used as visual evidence. An initial native fixture quit issue was fixed by terminating only after worker restoration completes.

## Not yet verified on the desktop

- Actual typing into both apps and keyboard-shortcut delivery/conflict handling.
- Drag-to-reorder, searchable-picker interaction, and the rename dialog through human input.
- Multiple live windows from one process and duplicate titles (covered by fake-backend tests only).
- Dialogs, fullscreen, Spaces changes, manual target movement, and app-imposed minimum sizes across diverse apps.
- Live target exit, unresponsive targets, denied/revoked permission recovery, or restoration retries through the native prompt.
- Native reconnection after application restart and ambiguous identifiers (unit-tested matching only).
- Multiple monitors, Intel Macs, macOS versions other than this host, long-running/load behavior, and crash recovery.

These results establish a working local docking prototype for the tested pair, not broad desktop compatibility. Original recovery snapshots are in memory; Force Quit is not a restoration path.

## Picker attachment panic fix — 2026-09-10

Reported failure: `RefCell already borrowed` in `windowDidBecomeKey:` when attaching through the picker. Dismissing the key panel synchronously reactivates the manager; the previous Attach handler still held an immutable UI borrow while the focus callback requested a mutable borrow. The earlier integrated test sent Attach directly to the worker, bypassing this UI path.

The Attach action now snapshots its command and panel handle, releases the UI borrow, and then hides the panel. Picker presentation and manager exit also release the borrow before changing window visibility. The key-window callback sets a separate `Cell` flag, which the normal timer consumes. The integrated test now exercises native picker presentation, selection, and the shared Attach action.

Verification:

- Formatting, Clippy with warnings denied, 19 unit tests, locked debug build, release package, and ad-hoc signature validation passed.
- Focused `--picker-smoke`: both native picker attachments completed without panic; restoration completed and process exit code was 0.
- Final readback matched this run's original states: Discord `(76, 160, 1233, 1091)`, minimized; Telegram Lite `(130, 167, 1215, 884)`, not minimized.
- The broader `--dock-smoke` rerun got past both picker attachments, then failed after three switches with `Could not hide previous window: Window did not accept minimization change; rollback: Ok(())`. It exited with code 1 and both original states were confirmed restored. This separate intermittent minimization issue remains unresolved; this rerun does not repeat the earlier full-switching success claim.

## Blanking and dragged-window ownership — 2026-09-10

The old opaque backdrop could cover an attached app while AppDock was foreground and the Accessibility worker had not raised the target yet. The window is now non-opaque, with a custom native content view painting only the controls while attached. The empty workspace retains its normal background. Activation no longer adds the extra 250 ms geometry debounce before requesting a raise.

Manual movement now waits for mouse release and restores the docking frame without dropping tab ownership. Add filters canonical AX aliases, and the backend independently rejects duplicate attachment. Invalid AX reads are checked against the current application window list before closure; unique identifiers can rebind replaced AX objects. A disconnected app must use Replace window or release its old tab before Add can create another tab.

Validation:

- Formatting, Clippy with warnings denied, 21 unit tests, and locked release packaging passed.
- `--surface-smoke` passed native bitmap assertions: transparent app-area pixels and opaque controls; it exited normally. The internal bitmap is used only for alpha assertions, not as proof of accurate composited control rendering.
- `--movement-fixture` passed using a disposable native AppDock child window: movement remained untouched while the simulated mouse-down state was set, rediscovery preserved identity, duplicate attachment was rejected, mouse release returned the window, and original-state restoration readback passed.
- The existing running AppDock instance and user application windows were not stopped or moved for these tests.

The fixture moves through AX and controls the pointer-state input; it does not synthesize a physical drag or trackpad scroll. The exact reported scrolling gesture still needs interactive confirmation. Fullscreen, dialog and permission interruptions continue to pause control. The previously recorded intermittent minimization issue is separate and remains unresolved.

## Release tab removal and drag latency — 2026-09-10

Release now removes the tab and attachment before restoration IPC. If restoring the original frame or minimized state fails, the window remains released and is never snapped back. Its snapshot is retained separately for AppDock → Retry restoration. The UI receives detached state before potentially slow AX calls. Closing still attempts any outstanding restoration.

Manager movement now uses a waking/coalescing mailbox and position-only AX writes. The 100 ms per-command sleep was removed; position-only requests no longer resize, wait for settling, read the entire target state, or write workspace JSON on every movement. Geometry writes are debounced and lifecycle polling resumes after movement settles. The API remains asynchronous, so some compositor/application lag is possible.

Validation:

- 26 tests passed, including release despite restoration failure, no re-docking after release, retained recovery/retry, position-only movement preserving actual size, latest-frame prioritization, and mailbox wakeup.
- Formatting, Clippy with warnings denied, and locked builds passed; local release package rebuilt and signature verified.
- The disposable native movement fixture issued 20 position-only updates in 2.1 ms (0.10 ms/update) on this host, verified the final frame, and passed original-state restoration readback.
- This timing measures AX request processing against the native fixture, not compositor frame timing or a physical mouse drag in Discord. The user's running instance was not stopped or reloaded.

This release behavior supersedes the earlier failed-release behavior described above: recovery no longer keeps the released tab visible.

## Disconnected Release target and native drag tracking — 2026-09-10

The hollow-circle entry is a disconnected saved tab. The earlier release change fixed backend detachment but missed the UI's empty action target after restart: it inherited only the backend's live selection, so Release could do nothing. UI actions now resolve an existing saved tab even without a live selection. Every tab also has an explicit × control targeting its own ID.

Online research identified the missing drag scheduling layer. [Apple's run-loop guide](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/Multithreading/RunLoopManagement/RunLoopManagement.html) documents event-tracking mode during mouse dragging and that timers outside the active mode do not fire. The earlier AX timing fixture did not exercise this native loop. A separate nominal 120 Hz sampler now runs in event-tracking mode, reads AppDock's own WindowServer bounds, and submits changed frames. It sleeps outside dragging; the ordinary UI timer remains separate.

Verification:

- 29 unit tests passed, including disconnected-only action targeting, stale selection fallback and explicit disconnected selection over a live tab.
- Native `--disconnected-smoke` passed: a disconnected tab was removed through the same Release helper with no active window.
- Native `--tracking-smoke` passed with 15 WindowServer samples in a 150 ms nested event-tracking loop on the final timer implementation. This verifies scheduling and live bounds retrieval; it does not measure physical mouse-to-compositor latency in Discord.
- Formatting, Clippy with warnings denied, locked debug/release builds, and ad-hoc package verification passed.
- All native checks used isolated AppDock fixtures; the user's running instance and application windows were not modified.

## Compact Terminator-style UI and inline rename — 2026-09-10

The main toolbar was replaced with a compact tab strip: + adds windows and × releases them. Rename and Close AppDock buttons are gone. Resume appears only when docking is paused; Allow window control appears only when permission is unavailable; Replace window appears for the selected disconnected tab. Standard window close and Cmd–Q still restore managed windows before quitting.

Styling uses the current Terminator default colors from `terminator/crates/core/src/appearance.rs`, 13-point Inter typography, 220×32-point flat tabs, and the current selected-tab underline color. Inter Regular and its original license were copied into AppDock and registered process-locally. Terminator files and user configuration were not changed.

Double-clicking a tab opens an inline text field. Enter/blur commits, Escape cancels. Native testing caught a redundant focus/selection call that prematurely ended editing; it was removed before delivery. A native double-click event now opens the editor and the isolated fixture verifies commit and cancel results.

Checks:

- 29 tests passed; formatting, Clippy with warnings denied, locked builds and release packaging passed.
- Native inline rename fixture passed commit and cancel.
- Native transparent-surface fixture passed with the new 64-point header.
- Native tracking-mode fixture passed with 14 WindowServer samples in 150 ms.
- [Native styled preview](screenshots/terminator-style-workspace.png) was captured by exact window ID and inspected. It contains dummy disconnected tabs, so Replace window is visible. No user app windows were attached or moved.

The screenshot helper's application-name activation stalled; it was replaced with exact-window capture. The preview and test processes exited normally. Physical keyboard/gesture use against real managed applications remains an interactive check; fixture double-click delivery is synthetic.
