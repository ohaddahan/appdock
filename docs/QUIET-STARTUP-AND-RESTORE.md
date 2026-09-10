# Quiet startup and minimized-window readiness — 2026-09-10

As approved, **Keep Apps Open on Close** is enabled by default. Used windows return to their original geometry but retain their current visibility, avoiding the minimize/reopen cycle. Settings can restore the old close behavior. Explicit Release still restores the original minimized state. Startup registers all matching tabs but only restores/focuses the first selection; other tabs restore when clicked. Never-selected startup windows remain untouched, including external changes made while AppDock is running.

A native delayed-readiness target reproduced the minimized-window failure: the window unminimized, then the old single check rejected its temporarily unavailable main-window controls and rolled it back. [Failing reproduction](review-evidence/restore-readiness-reproduction/RestoreReady.log). The initial attempt did not exercise its delay hook and is retained separately as inconclusive. Selection and Resume now wait for readiness with a cancellable 900 ms polling budget. Native errors retain their codes; failed preparation/rollback keeps the original recovery snapshot. No system-wide Dock settings were changed.

[Six targeted native cases pass](review-evidence/quiet-startup-restore/results.json): RestoreReady, Minimized, Animations, Startup, settings, and A4. The delayed target now docks and releases correctly. Settings verifies that the default can be disabled and that the choice persists.

One three-window fixture measured:

| Operation | Time |
| --- | ---: |
| Previous sequential startup | 2,024 ms |
| Previous original-state close | 2,030 ms |
| Startup restoring only the first tab | 647 ms |
| Keep-open close after using all tabs | 269 ms |
| Reopen with those windows already open | 129 ms |

[Timing log](review-evidence/quiet-startup-restore/Animations.log). These are local disposable-window measurements, not a guarantee for every app. WhatsApp/Spotify themselves were not manipulated or independently revalidated.

**81 deterministic tests pass**, including delayed readiness, timeout/cancellation/error propagation, deferred-window ownership, incomplete startup rollback recovery, keep-open close, explicit release semantics, and persisted opt-out. Formatting, strict Clippy, locked build, Rust 1.95 checks, and diff whitespace checks pass. [Test log](review-evidence/quiet-startup-restore/tests.log), [Clippy](review-evidence/quiet-startup-restore/clippy.log), [build](review-evidence/quiet-startup-restore/build.log), [Rust 1.95](review-evidence/quiet-startup-restore/msrv.log).
