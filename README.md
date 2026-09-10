# AppDock

A standalone native macOS workspace for **existing** application windows. Rust 2024, AppKit via `objc2`, public Accessibility APIs, and `global-hotkey`. No webview or embedded browser. Terminator is a separate application and is not a dependency.

AppDock provides a backdrop and tab strip. The selected external window sits below the controls; its own title bar and Dock entry remain. Switching restores and focuses that exact window and minimizes the previous window. Apps remain running.

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

- The **+** button opens the searchable picker. Only windows supporting required operations appear. Rows include app, title, process ID, and session-local window ID to distinguish duplicate titles. Refresh updates the list. Managed windows are excluded even if Accessibility returns a second handle. If an app has a disconnected tab, use Replace window (or release the old tab) before adding another window for that app. Apps are never launched automatically.
- Click a tab to switch. Drag a tab horizontally to reorder. Double-click a tab to rename it inline, for example “Discord - username 1”. Enter or clicking away saves; Escape cancels. Icons come from the running app.
- The **×** button on a tab removes that tab and restores the window's original geometry and minimized state. Apps are not quit. Disconnected tabs can be removed too, including immediately after restart. The tab is removed before restoration runs. If restoration fails, it stays detached and its snapshot is retained separately; use **AppDock → Retry restoration** from the menu.
- A hollow-circle tab is disconnected. Select it and use **Replace window**. Saved tab labels are not window identities.
- Moving or resizing AppDock repositions the selected window. Dragging uses immediate position-only requests; geometry saves and lifecycle polling are kept out of the continuous movement path. A target can enforce a larger minimum size; AppDock accepts the actual returned size. The external window may then extend beyond the backdrop. The app area is transparent while attached, so activating or scrolling AppDock’s controls cannot cover it with a blank background.
- Dragging a managed window keeps it attached and returns it to the docking area when you release the mouse. Use the tab’s **×** to detach it. Minimizing it, entering fullscreen, opening a detected dialog, or changing Spaces pauses docking; resolve the interruption and click **Resume**. AppDock operates on one desktop at a time.
- The standard red window button or Cmd–Q closes AppDock and restores attached windows. Failed restoration keeps the application open with a retry action. Force Quit, crashes, and machine shutdown cannot guarantee restoration; original snapshots exist only for the current process lifetime.

Separate account profiles are deferred. Tabs can organize account windows or instances that an app already exposes; AppDock does not create isolated profiles.

## State and shortcuts

Version 1 JSON is saved atomically to:

```text
~/Library/Application Support/AppDock/workspace.json
```

`APPDOCK_DATA_DIR` selects an isolated state directory. A file lock prevents two managers from opening the same workspace. Unknown versions, malformed state, and duplicate tab IDs are rejected without replacing the original JSON.

Saved data includes labels, order, bundle identifiers, optional Accessibility identifiers, window geometry, and shortcuts. Live handles and restoration snapshots never go in workspace JSON. Reconnection requires a nonempty identifier unique among both saved tabs and eligible live windows for that bundle. Without one, explicitly select a replacement. Titles alone never reconnect a window. Reconnection binds a handle; selecting the tab starts docking.

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

`--dock-smoke` opens the native picker and invokes the same attachment action as the UI for each target, then drives the AppKit manager and worker through twelve switches, manager movement/resizing, release, and quit. It needs an empty isolated workspace and the same two running apps. Success is reported by the stage messages and a normal exit; timeouts print a failure before restoration. Keep its log when validating.

`--picker-smoke` uses the same isolated-workspace prerequisites as `--dock-smoke`, but stops after attaching both apps through the native picker and restoring them on exit. It specifically checks picker dismissal and synchronous key-window callbacks.

An isolated startup/quit smoke test can capture AppKit's internal view bitmap:

```sh
APPDOCK_DATA_DIR="$(mktemp -d /tmp/appdock-ui.XXXXXX)" \
APPDOCK_SMOKE_SCREENSHOT=/tmp/appdock-view.png \
  ./target/debug/appdock --ui-smoke
```

Internal bitmap capture does not reliably render composited native controls. Use a macOS window screenshot for visual evidence. `APPDOCK_SMOKE_TICKS=200` keeps this fixture visible for about 30 seconds before it closes.

The appearance follows Terminator’s default flat dark palette, 13-point Inter typography, compact tabs and selected-tab underline. Resume and permission controls appear only when needed. See the [native preview](docs/screenshots/terminator-style-workspace.png).

See [validation results](docs/VALIDATION.md), [native screenshot](docs/screenshots/native-workspace.png), and [architecture](docs/ARCHITECTURE.md).

`--surface-smoke` uses the same environment as `--ui-smoke` and asserts transparent app-area pixels with opaque controls. `--movement-fixture` creates a disposable native AppDock child, verifies movement retains ownership and rejects duplicate attachment, then checks return-to-dock and restoration. It does not operate on your existing app windows.

`--tracking-smoke` requires an isolated `APPDOCK_DATA_DIR` and checks the drag sampler in a native event-tracking run loop. `--disconnected-smoke` seeds a disposable disconnected tab and verifies Release removes it without an active app window. Neither fixture attaches user app windows.

`--rename-smoke` uses an isolated workspace, dispatches a native double-click to a tab, and verifies inline rename commit/cancel. `--design-smoke` shows two dummy tabs for visual review and accepts the same screenshot/timeout variables as `--ui-smoke`.
