# AppDock code review

Reviewed on 2026-09-10. These are the fixes I would prioritize.

1. **High — A closed window can prevent AppDock from quitting.** If release restoration fails and that window subsequently closes, its recovery record survives indefinitely. Every quit retries an impossible restoration. Clear recovery records after confirming closure. See [src/engine.rs](src/engine.rs), line 212.

2. **Medium — Adding an app can leave the wrong tab highlighted.** The backend selects the newly attached window, but the UI preserves the previous `editing` tab and uses it for highlighting. Separate the active-window indicator from the rename/action target. See [src/ui.rs](src/ui.rs), line 1041.

3. **Medium — Release can discard the final workspace position.** The mailbox deletes pending resize commands, while the UI remembers that geometry as already sent. Remaining windows can stay at the old position until another geometry change. Preserve the latest requested geometry independently of cancelled movement commands. See [src/worker.rs](src/worker.rs), line 94, and [src/ui.rs](src/ui.rs), line 816.

4. **Medium — Resume leaves inactive windows behind.** Move or resize AppDock while paused, then resume: only the selected window returns to the workspace. Other windows’ old positions become their accepted state, so polling never corrects them. Reposition all previously docked windows during resume. See [src/engine.rs](src/engine.rs), line 353.

5. **Medium — Accessibility timeouts are applied too late in some paths.** Discovery reads `AXSubrole` before setting the window timeout; replacement handles also lack timeout initialization. Apple specifies that these timeouts apply to individual objects. Initialize them before reading attributes, and make background discovery interruptible so release/quit commands can proceed promptly. See [src/macos.rs](src/macos.rs), line 264, and [Apple documentation](https://developer.apple.com/documentation/applicationservices/1459345-axuielementsetmessagingtimeout?changes=l_4).

6. **Medium — Fast keyboard navigation loses steps.** Every queued shortcut calculates its destination from the same backend selection snapshot. Two quick “next” presses can therefore request the same tab twice. Calculate from the latest requested selection. See [src/ui.rs](src/ui.rs), line 1561.

For maintainability, I would move the native smoke-test machinery out of the 2,025-line [src/ui.rs](src/ui.rs), introduce typed errors for closed windows versus timeouts versus permission loss, and add macOS CI.

**Validation:** All 37 existing tests, Clippy, and formatting pass. Four additional checks in a temporary copy reproduce the state/queue problems in items 1–4; items 5–6 come from code/API inspection. No application source files changed during the review, and no live-window tests were run. Line numbers refer to the code inspected during this review.
