# AppDock

A standalone native macOS workspace for **existing** application windows. Rust 2024, AppKit via `objc2`, public Accessibility APIs, and `global-hotkey`. No webview or embedded browser. Terminator is a separate application and is not a dependency.

AppDock provides a backdrop and tab strip. The selected external window sits below the controls; its own title bar and Dock entry remain. Switching raises and focuses that exact window above an opaque backdrop, which covers the inactive tabs. Tab switches do not minimize windows; apps remain running.

## Run locally

Requires macOS 12+ and Rust 1.95+. The current build was exercised on Apple Silicon; other systems are not yet validated.

```sh
cd appdock
cargo build --locked
scripts/package.sh           # release bundle, ad-hoc signed
open dist/AppDock.app
```

`./scripts/package.sh --debug` packages the development build. Neither form notarizes, publishes, nor installs outside this directory.

If prompted, click **Allow window control…**, enable AppDock in System Settings → Privacy & Security → Accessibility, and select **Resume**. If AppDock is absent from the list, use the + button to select `dist/AppDock.app`. Permission belongs to the launch context: a successful terminal diagnostic does not prove a Finder-launched bundle is authorized. Rebuilding an ad-hoc signed app may require granting permission again.

## Use

- The **+ Add App** button beside the AppDock title opens a searchable picker inside the workspace. It uses AppDock’s dark palette and flat selectable rows, with no separate window or OS dropdown. Rows show the app and window title; process/window IDs are available in tooltips. Double-click a row to attach it directly, or select it and use Attach window. Arrow keys select a result, Enter attaches it, and Escape or Cancel returns to the workspace. Minimized windows appear with a Minimized label and are restored when added; their docking capabilities are rechecked after restoring. Other windows must support the required operations. Refresh updates the list. Managed windows are excluded even if Accessibility returns a second handle. If an app has a disconnected tab, use Replace window (or release the old tab) before adding another window for that app. Apps are never launched automatically.
- **Settings**, beside **+ Add App**, opens startup app choices. Individual checkmarks save automatically. **Save Current Apps for Startup** replaces the saved choices with the currently attached apps; **Reset Saved App Choices** clears those choices without releasing current tabs or resetting geometry/shortcuts. You can also use **AppDock → Settings…** or **⌘,** while AppDock is active. The existing **AppDock → Auto-add at Startup** menu remains available.
- New tabs use only the app name. Tabs have outlined backgrounds and a brighter active-tab accent. They remain compact and single-line; long custom names truncate. Click to switch, drag horizontally to reorder, or double-click to rename. Activating a docked app window directly brings AppDock’s frame immediately behind it, keeping the frame visible while mouse and keyboard input go to the app. Explicit AppDock minimization is respected. Ctrl+A and Cmd+A select the full rename text. Enter or clicking away saves; Escape cancels. AppDock takes keyboard focus for renaming and suspends external app-raising until editing ends. Icons come from the running app.
- The **×** button on a tab removes that tab and requests the window's original geometry and minimized state. If the app or desktop constrains the size or position, AppDock accepts the resulting frame and reports the adjustment. Apps are not quit. Disconnected tabs can be removed too, including immediately after restart. The tab is removed before restoration runs. If restoration fails, it stays detached and its snapshot is retained separately; use **AppDock → Retry restoration** from the menu.
- Tabs mirror the app’s Dock notification badge: a count (capped at 99+) or a red dot. Updates are best-effort, about every two seconds. Badges are app-wide, so multiple windows of one app may show the same badge. Clicking a tab does not clear it; the badge follows the app’s Dock state. Apps that do not expose a Dock badge have no indicator, which does not guarantee there is no pending action.
- A hollow-circle tab is disconnected. Select it and use **Replace window**. Saved tab labels are not window identities.
- Moving or resizing AppDock repositions all windows that have been docked. Dragging uses immediate position-only requests; geometry saves and lifecycle polling are kept out of the continuous movement path. An 8-point AppDock frame surrounds the selected window. If an app enforces a larger minimum size, the manager grows to fit its actual size inside the frame. The normal tab strip is 36 points high; an extra status row appears only for errors or required actions. The selected app stays above the manager window so clicks reach it directly. The controls window keeps a transparent app area; a separate opaque backdrop stays below both windows and covers inactive windows, including larger minimum sizes.
- Dragging a managed window keeps it attached and returns it to the docking area when you release the mouse. Use the tab’s **×** to detach it. Minimizing it, entering fullscreen, opening a detected dialog, or changing Spaces pauses docking; resolve the interruption and click **Resume**. AppDock operates on one desktop at a time.
- The standard red window button or Cmd–Q closes AppDock and restores attached windows. Window-control failures keep the application open with a retry action; a size or position mismatch alone does not block closing. Successfully released tabs are removed from saved state. Force Quit, crashes, and machine shutdown cannot guarantee restoration; original snapshots exist only for the current process lifetime.

Separate account profiles are deferred. Tabs can organize account windows or instances that an app already exposes; AppDock does not create isolated profiles.

## State and shortcuts

Version 1 JSON is saved atomically to:

```text
~/Library/Application Support/AppDock/workspace.json
```

`APPDOCK_DATA_DIR` selects an isolated state directory. A file lock prevents two managers from opening the same workspace. Unknown versions, malformed state, and duplicate tab IDs are rejected without replacing the original JSON.

By default, every launch starts with **no attached apps**, including `cargo run`. Opt in per app through **Settings** beside **Add App** to add detected windows automatically. Startup validates the saved workspace, clears any previous session’s tabs before window discovery, and saves that empty tab list. Window size, position, keyboard shortcuts, and startup app choices are preserved. A saved Terminal window is never automatically reattached just because it has a stable Accessibility identifier.

Tabs, names and order belong to the current session. Add windows using **+ Add App**, or check an app in **AppDock → Auto-add at Startup**. Each saved startup rule matches an exact application bundle ID after the initial discovery and workspace geometry are ready. A single available window is added, including a minimized window. If an app has several windows, AppDock leaves the choice to you and shows a status message; apps that are not running are not launched. Manual interaction cancels remaining startup work. Live handles and restoration snapshots are never loaded from workspace JSON. Later discovery only refreshes the picker, so releasing a startup app does not immediately reattach it. Uncheck an app in the menu to stop adding it on future launches. The lifecycle observer removes confirmed closed windows and persists that cleanup.

Edit these JSON keys while AppDock is closed to configure shortcuts:

```json
{
  "next_shortcut": "Control+Alt+Super+ArrowRight",
  "previous_shortcut": "Control+Alt+Super+ArrowLeft"
}
```

`Super` is Command on macOS. Shortcuts register only while AppDock or a managed application is foreground. Foreground eligibility is checked every 150 ms; registration conflicts are shown in the status line.

## Verify

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-features --locked
cargo build --locked
./target/debug/appdock --diagnose
```

`--diagnose` is read-only and reports Accessibility status, window counts, and geometry for Discord and Telegram Lite. `--smoke` is an explicit **live-window test**: it requires exactly one eligible window in each app, switches six cycles, moves/resizes windows, restores original state, and reads it back. Do not interact with the targets during that test. It does not type or send messages.

```sh
./target/debug/appdock --smoke
APPDOCK_DATA_DIR="$(mktemp -d /tmp/appdock-test.XXXXXX)" \
  ./target/debug/appdock --dock-smoke
```

`--dock-smoke` opens the inline picker and invokes the same attachment action as the UI for each target, then drives the AppKit manager and worker through twelve switches, manager movement/resizing, release, and quit. It needs an empty isolated workspace and the same two running apps. Success is reported by the stage messages and a normal exit; timeouts print a failure before restoration. Keep its log when validating.

`--picker-smoke` uses the same isolated-workspace prerequisites as `--dock-smoke`, but stops after attaching both apps through the native picker and restoring them on exit. It checks inline picker presentation and attachment through the normal UI action.

An isolated startup/quit smoke test can capture AppKit's internal view bitmap:

```sh
APPDOCK_DATA_DIR="$(mktemp -d /tmp/appdock-ui.XXXXXX)" \
APPDOCK_SMOKE_SCREENSHOT=/tmp/appdock-view.png \
  ./target/debug/appdock --ui-smoke
```

Internal bitmap capture does not reliably render composited native controls. Use a macOS window screenshot for visual evidence. `APPDOCK_SMOKE_TICKS=200` keeps this fixture visible for about 30 seconds before it closes.

The appearance follows Terminator’s dark palette and 13-point Inter typography, with outlined tab backgrounds and a brighter selected-tab underline. Resume and permission controls appear only when needed. See the [native preview](docs/screenshots/terminator-style-workspace.png).

See [validation results](docs/VALIDATION.md), [native screenshot](docs/screenshots/native-workspace.png), and [architecture](docs/ARCHITECTURE.md).

`--overlay-fixture` creates two disposable windows in a child process and checks twelve same-app tab switches, actual WindowServer ordering around the opaque backdrop, absence of minimization, movement/resizing, and restoration. It does not attach existing user app windows.

`--surface-smoke` uses the same environment as `--ui-smoke` and asserts transparent app-area pixels in the controls window with opaque controls. `--movement-fixture` creates a disposable native AppDock child, verifies movement retains ownership and rejects duplicate attachment, then checks return-to-dock and restoration. It does not operate on your existing app windows.

`--tracking-smoke` requires an isolated `APPDOCK_DATA_DIR` and checks the drag sampler in a native event-tracking run loop. `--disconnected-smoke` seeds a disposable disconnected tab and verifies Release removes it without an active app window. Neither fixture attaches user app windows.

`--rename-smoke` uses an isolated workspace, dispatches a native double-click, and verifies AppDock keyboard focus, Ctrl+A/Cmd+A, typing, and inline rename commit/cancel. `--design-smoke` shows two dummy tabs for visual review and accepts the same screenshot/timeout variables as `--ui-smoke`.

`--restoration-fixture` extends the disposable overlay fixture with deliberately unattainable restoration dimensions; it verifies that native size constraints do not block close or retain stale tabs. `--cleanup-smoke` opens an isolated workspace seeded with saved tabs, including Terminal with a stable identifier, and verifies startup clears both UI and saved state without creating live attachments.

`--frame-smoke` uses an isolated workspace and two disposable child windows to test frame fitting around native minimum sizes, inline picker search/Cancel/Attach, rename keyboard ownership despite queued Raise requests, twelve tab switches, AppDock reveal without keyboard-focus transfer, manager movement/resizing and restoration. It does not attach existing user windows.

See the [inline window-picker preview](docs/screenshots/inline-window-picker.png).

`--pointer-smoke` uses two disposable app windows and macOS mouse-down target queries to verify that app-area clicks go to the app and tab-strip clicks go to AppDock, including after manager movement/resizing. It does not inject clicks into existing applications.

`--diagnose-badges` reads Dock badge metadata for the supported diagnostic app names without reading messages or modifying applications. See the [tab badge preview](docs/screenshots/tab-notification-badges.png), using dummy count/dot values.

### Review regression fixtures

`python3 scripts/review-native.py --output /tmp/appdock-review-results` runs the review cases and affected existing fixtures serially using disposable child windows and fresh temporary workspaces. Build with `cargo build --all-features --locked` first. Select cases with `--cases A1,A3,A4,D3,D6`, or use `--review-fixture A3` directly with a fresh `APPDOCK_DATA_DIR` containing no `workspace.json`.

Accessibility must be available to the launched binary; a missing prerequisite is reported as a failure. The runner records case outcomes, durations, exit codes, logs, and host details and cleans up its own processes. See [the review tracker](docs/REVIEW-TRACKER.md) for regressions and validation limits. Hosted `macos-15` CI runs deterministic checks, including Rust 1.95 compatibility; native fixtures remain local.

### Startup and minimized-window checks

`--review-fixture Startup` checks saved rules over repeated launches, automatic restoration of a minimized window, release without immediate reattachment, and disabling a rule. `--review-fixture Minimized` starts discovery with fresh AX handles after all disposable windows are minimized, then verifies attachment and restoration. Both require an isolated `APPDOCK_DATA_DIR`. Run them together with `python3 scripts/review-native.py --output /tmp/appdock-startup-review --cases Startup,Minimized`.

See [startup, tab, and minimized-window validation](docs/STARTUP-AND-MINIMIZED-WINDOWS.md) for reproduction logs and native validation limits.

### Settings and docked frame appearance

The Settings button waits for the worker focus barrier before opening its native menu. This keeps pending app-raise requests from taking focus during configuration. Saving/resetting affects startup app choices only. AppDock disables its own shadow while an app is docked and uses the surrounding frame color behind rounded native corners; the external app retains its native border and shadow. See [settings and frame validation](docs/SETTINGS-AND-FRAME.md).
