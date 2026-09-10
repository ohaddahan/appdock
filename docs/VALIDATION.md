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


## Opaque backdrop switching — 2026-09-10

Tab switching no longer minimizes the previous window. A separate opaque normal-level window covers inactive tabs and stays below the selected app. The controls retain their transparent app area; their key-window callback keeps the backdrop below the app during editing. All docked windows and the backdrop follow workspace movement. Release and exit still restore original geometry and minimized state.

- 33 unit tests passed, including exact-handle switching without any minimization call, restoring an initially minimized target once, inactive-window movement/resizing, dialog preflight, and coverage of larger windows on negative desktop coordinates.
- Formatting, Clippy with warnings denied, locked builds and local release packaging/signature verification passed.
- Native `--overlay-fixture` passed twelve switches between two duplicate-title windows in a disposable child process. WindowServer readback confirmed selected window above cover above inactive window. Both targets remained unminimized; movement/resizing and original-state restoration passed. Activating the disposable controls retained keyboard focus while the cover stayed behind the selected target.
- The fixture caught AX focus settling before WindowServer order. Cached window numbers now wait for their own window to reach the front and cannot be reassigned to another retained AX window.
- Native surface, inline rename commit/cancel, and tracking-mode checks passed; the tracking fixture recorded 14 samples in 150 ms.

These checks used disposable windows and isolated temporary workspaces. Existing user application windows were not attached, moved, or minimized; physical gestures and switching in Discord/Telegram were not revalidated. The prior switch-minimization failure path is removed. Already running AppDock instances require a restart to use the rebuilt bundle.

## Stale-session cleanup and nonblocking geometry restoration — 2026-09-10

A read-only check of the saved workspace found three tabs, including two Discord entries without AX identifiers. Such entries could not reconnect but remained visible indefinitely. Startup now prunes unreconnectable entries after discovery; periodic discovery and confirmed AXWindows closure checks remove stale tabs and persist the result. Successful release on close removes saved tabs too. Live attachments, incomplete reads and temporarily ineligible matching windows are preserved.

The reported Retry close message originated from the exact-frame comparison in restoration, after geometry operations had succeeded. Release/close now accepts valid adjusted geometry, verifies minimized state and logs the requested/actual frames. True window-control errors retain retry behavior; switch rollback remains strict. This identifies the failing code path, not the exact app constraint or timing behind the user’s original snapshot.

- 39 unit tests passed, including constrained restoration, actual control failure/retry, startup cleanup with label retention, incomplete-read preservation, periodic stale cleanup and avoiding writes for unchanged windows.
- Native `--restoration-fixture` passed with deliberately unattainable original dimensions on two disposable native windows. Close completed, tabs and recovery snapshots cleared, and final restoration to the fixtures’ actual original geometry passed. The same run also passed the twelve-switch overlay and controls-focus checks.
- Native `--cleanup-smoke` removed two seeded stale entries from an isolated workspace and verified the empty tab list was persisted.
- Formatting, Clippy with warnings denied, locked builds, release packaging and signature verification passed.

These tests did not attach or change existing user windows or edit the real saved workspace. The fixes take effect after restarting the rebuilt app.

## Fresh launches and Add App beside the title — 2026-09-10

The earlier startup cleanup intentionally retained reconnectable tabs; `cargo run` reads the same workspace as the bundled app, so Terminal could reappear without being explicitly added. Startup now clears **all** previous-session tabs before worker creation, preserves geometry/shortcuts and persists the empty state. Discovery no longer has any automatic attachment path. This supersedes the earlier selective startup-cleanup behavior.

The control now reads **+ Add App**, has a visible native border/background, and sits immediately beside the AppDock label in a left title-bar accessory. [The native empty-workspace preview](screenshots/titlebar-add-app.png) was captured and inspected. Accessibility Press was followed by fresh window/state reads confirming the “Choose an existing window” picker and focused search field; no Attach action was performed. The disposable preview exited normally.

- 35 tests passed. Obsolete automatic-reconnection tests were replaced by a fresh-launch regression that clears a stable Terminal identity while preserving geometry and shortcuts; unsupported workspace versions remain untouched.
- Native startup smoke passed with saved Terminal and stale-app tabs: no UI tabs, live attachments, or backend selection, and the cleared file was read back.
- Formatting, Clippy with warnings denied, locked build, local release packaging and signature verification passed.
- Existing user app windows and the real workspace file were not modified by these checks. Both the next `cargo run` and the rebuilt bundle use the fresh-session behavior.

## Readable Add App and empty-state layout — 2026-09-10

Add App now uses an explicit white attributed title for normal/alternate states and a lighter bezel background. The idle rename/reorder status is hidden when there are no tabs; permission and error status still appear when relevant. Empty-state instructions sit 24 points below the app-area header and remain anchored to the top during resizing.

- Formatting, Clippy, locked build and 35 existing tests passed; local release bundle rebuilt and signature verified.
- A disposable empty native workspace was [captured and inspected](screenshots/readable-empty-workspace.png): white Add App text, absent idle status and instructions near the top.

## Compact tabs, wrapping frame and rename keyboard ownership — 2026-09-10

New tabs use the app name, with 160-point single-line tabs and truncated custom labels. The idle rename/drag text is removed, and the normal header shrinks to 36 points. Status/actions expand it only when needed. An 8-point AppDock frame surrounds the selected app; the manager grows around app-enforced minimum sizes while keeping the native app window independent.

Renaming explicitly activates AppDock and uses a dedicated field editor supporting Ctrl+A/Cmd+A. An atomic focus gate and worker barrier cancel queued app raises and wait for earlier focus IPC before presenting the editable field. Ending editing releases the gate. Native validation caught synchronous field-editor requests reentering UI state; routing now uses independent Cell/OnceCell state.

- 36 unit tests passed; app-name defaults and cancellation/ordering of pending focus commands are covered.
- Final native rename smoke passed AppDock activation, Ctrl+A/Cmd+A selection, typed replacement, commit and cancel.
- Final `--frame-smoke` passed with two disposable native child windows: picker attachment, automatic manager growth from a 400-point-tall workspace to fit the app, rename focus despite Raise requests before/after starting rename, twelve switches, manager movement/resizing, release and close restoration.
- Surface bitmap checks confirmed a transparent app area with opaque surrounding frame and controls. Tracking-mode smoke recorded 15 WindowServer samples in 150 ms.
- Formatting, Clippy with warnings denied, locked builds and release packaging/signature verification passed.

These fixtures did not attach or type into existing user applications. Physical keyboard use in the user's Telegram instance was not exercised; native event delivery used disposable windows through the real AppKit/worker paths.

## Inline themed picker and automatic AppDock reveal — 2026-09-10

Replaced the separate native picker panel/dropdown with an inline, palette-matched search/list view. A captured [disposable preview](screenshots/inline-window-picker.png) was inspected: the list, search and actions appear within the AppDock window. The preview contains only disposable child windows.

Direct activation of a cached docked window now reveals AppDock using front ordering without keyboard activation. Matching uses the exact known window number and owner PID; unrelated same-app windows do not qualify. Explicit AppDock minimization is respected.

- 36 unit tests, formatting, Clippy with warnings denied and locked build passed.
- Final native integration passed picker keyboard ownership, search with no results, Cancel/reopen, selection and attachment of both child windows, rename focus, twelve switches, automatic reveal without keyboard-focus transfer, manager resize, release and close restoration.
- Initial reveal validation sampled focus too late and observed Chrome foreground; another run lost target focus during a switch. The final fixture checks keyboard ownership immediately around the reveal call and validates WindowServer ordering on the following UI pass. Relative ordering was replaced with orderFrontRegardless to bring an inactive AppDock window forward reliably.
- Screen capture used already-granted permission and an exact disposable window number. No existing app windows were attached or changed.
- Local release packaging and signature verification passed. Physical interaction with the user's own docked apps remains outside these fixture checks.

## Attached-app mouse interaction — 2026-09-10

The automatic-reveal path had placed the full manager window above the external app. The fix keeps the selected app above AppDock, with the opaque backdrop behind both. The frame remains visible around the app, and the inline picker still covers it only while open. Rename keeps its keyboard editor while the manager is ordered below the app; losing key status commits the rename through a deferred callback.

- 36 unit tests, formatting, Clippy and locked builds passed.
- Native `--pointer-smoke` passed: macOS mouse-down target queries at three app-body points resolved to the exact external window; the tab-strip point resolved to AppDock. The checks passed again after manager movement/resizing, followed by release and restoration.
- Final `--frame-smoke` passed pointer routing while rename owned the keyboard, inline picker attachment, Ctrl+A/Cmd+A and typing, twelve switches, frame reveal without keyboard transfer, release and restoration.
- An initial attempt to exercise the old ordering stopped earlier in the fixture because AppDock did not own keyboard focus, so it did not produce a before-fix pointer readback. The new pointer checks pass with the corrected order.
- Local release packaging and signature verification passed. These checks use disposable windows and read native mouse targets; they do not send mouse events to existing user apps.

This supersedes the earlier validation of above-app/orderFrontRegardless reveal behavior: those keyboard and rendering checks did not verify mouse routing.

## Tab icon containment — 2026-09-10

The tab strip now has an 8-point inset from both window edges, matching the workspace frame. Tab buttons have an additional 4-point leading inset. Explicit clipsToBounds is enabled on the workspace surface, scroll view, clip view and tab document so icon drawing and horizontally scrolled tabs stay within the container. This changes drawing/layout only; the external-app window ordering and mouse routing are unchanged.

Formatting, Clippy with warnings denied, the locked build and all 36 existing tests passed. Local release packaging and signature verification passed. The user's exact Discord screenshot was not recaptured in this check.

## Tab notification badges — 2026-09-10

Added optional Dock-badge mirroring to tabs. Numeric labels render as a small count pill, capped at 99+; other nonempty labels render as a red dot. The original badge remains app-owned and app-wide. The app name retains its label; badge rendering reserves space before the close control.

- The live read-only Swift inspection and the implemented Rust reader both found Discord exposing a dot (`AXStatusLabel = •`). The final Rust diagnostic read Telegram Lite/Spotify with no badge and completed in 30 ms on this run.
- The [native view-bitmap preview](screenshots/tab-notification-badges.png) was inspected with dummy count 7 and dot values. Both fit inside the tab without overlapping the label or close button. This is not evidence of seven live unread Discord messages.
- 37 tests passed, including numeric/99+/dot/zero/empty formatting. Formatting, Clippy with warnings denied, locked builds, release packaging and signature verification passed.
- No messages, notification contents or app settings were read or modified. Badges are best-effort per app, not a universal per-window pending-action detector.

## Review fixes — 2026-09-10 (current working tree)

The current baseline was independently checked at **37 tests**, before implementing the review plan. The updated suite has **65 passing deterministic tests**. Formatting, Clippy with warnings denied, locked build, Rust **1.95.0** all-target/all-feature check, and diff whitespace checks pass. The earlier counts in this document describe older iterations.

All A1–A6 defects are fixed. Global minimization pause and focus-operation checks are retained with regression coverage. Monotonic refresh deadlines, private app-list updates, typed backend errors, fixture extraction, and hosted `macos-15` CI configuration are implemented. Both delayed frame settling and final-owner backdrop lifetime were reproduced natively before their production fixes.

The final full native run exercised **17 serial disposable-only cases**: 16 passed and D4 encountered a focus timeout during setup. D4 passed when rerun alone without a production change. After a final correction to retain closure evidence across preflight, all five affected native lifecycle/geometry/resume/discovery cases passed again. This intermittent native focus acquisition remains a validation limitation; it was not converted into a pass or hidden. The final frame fixture also checks an actual open rename editor against a changed selection snapshot and verifies picker/rename focus.

See [the review tracker](REVIEW-TRACKER.md) for every finding's disposition, exact regression names, pre-fix failures, native results, and remaining gaps. [Full native manifest](review-evidence/native-verified/results.json), [D4 rerun](review-evidence/native-focus-recheck/results.json), and [deterministic output](review-evidence/deterministic.log) retain actual evidence. Hosted CI has been configured but not executed. No existing user windows, real workspace, installation, or deployment were changed.

Run the local fixtures from a trusted desktop launch context:

```sh
cargo build --all-features --locked
python3 scripts/review-native.py --output /tmp/appdock-review-results
# Or select specific issues; each gets a new isolated directory:
python3 scripts/review-native.py --output /tmp/appdock-review-selected --cases A1,A3,A4,D3,D6
# Direct mode also requires a fresh isolated directory:
APPDOCK_DATA_DIR="$(mktemp -d /tmp/appdock-review.XXXXXX)" target/debug/appdock --review-fixture A3
```

The runner only allows disposable fixtures; it does not run the historical Discord/Telegram smoke modes. Missing Accessibility yields exit code 2 and a **prerequisite failure**, including for cases that could otherwise appear to pass without exercising native control. Existing smoke command names remain unchanged.

## Startup configuration, tab styling, and minimized discovery — 2026-09-10

The new baseline is **71 passing deterministic tests**, with formatting, strict Clippy, locked build, and Rust 1.95 checks passing. Startup apps can be selected through **AppDock → Auto-add at Startup**; choices survive release, quit, and relaunch. Multiple-window apps are left for manual selection. Tabs now have stronger boundaries and active styling.

A failing native reproduction found ordinary minimized windows exposing an `AXDialog` subrole and read-only `AXMain`. Discovery now includes these minimized windows and validates full docking capabilities after restoring them. Native tests cover discovery with fresh handles, startup across repeated launches, restoring original minimization, disabling saved rules, and the actual menu toggle.

See [the detailed feature validation](STARTUP-AND-MINIMIZED-WINDOWS.md) for before/after evidence, test manifests, and the retained intermittent picker-focus failure. WhatsApp/Spotify and other existing user windows were not changed. No installed app or deployment was modified.

## Visible Settings, saving/resetting apps, and corner framing — 2026-09-10

A visible title-bar **Settings** button now exposes automatic checkmark persistence, **Save Current Apps for Startup**, and **Reset Saved App Choices**. Saving/resetting preserves live attachments and other preferences. AppDock's own shadow is disabled while docked, and its backdrop color matches the surrounding frame.

**72 deterministic tests**, formatting, strict Clippy, locked build, and Rust 1.95 checks pass. Targeted native settings/surface/startup tests pass. Broader frame/pointer fixtures encountered focus/occlusion failures, and an exact before/after match for the supplied tiny corner crop remains unverified. [Details and evidence](SETTINGS-AND-FRAME.md).

## Quiet close/reopen and delayed minimized-window readiness — 2026-09-10

**81 tests and six targeted native cases pass**, along with formatting, Clippy, locked build, and Rust 1.95 checks. Keep Apps Open on Close is now the approved default; startup only restores the selected tab. A native delayed-control reproduction exposed premature rejection after unminimizing, fixed by a bounded cancellable readiness wait. Three-window fixture close time fell from 2,030 ms to 269 ms; reopening took 129 ms. [Evidence and limitations](QUIET-STARTUP-AND-RESTORE.md).

## Developer ID release workflow (2026-09-10)

Added signing and notarization to both native macOS release jobs using the five
Apple repository secrets. The workflow requires a valid matching Developer ID
Application identity, signs the executable and bundle with hardened runtime and
timestamps, requires Accepted notarization, and staples/validates the ticket and
checks Gatekeeper before archiving. Temporary credentials are cleaned up after
success or failure. Local packaging remains unchanged.

Validation: workflow YAML parsed, all 11 embedded shell blocks passed `bash -n`,
manual-only trigger assertion passed, all eight release revision scenarios passed
with mocked remote operations, and `git diff --check` passed. No local credentials
were imported, release triggered, or remote state changed. Actual certificate
import, notarization acceptance, hosted CI, and downloaded-app launch (including
Accessibility permission behavior) remain unverified.
