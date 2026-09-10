# Confirmed startup choices — 2026-09-10

The user's queued answers confirm that configured apps should contribute **all eligible windows**, that the corner artifact is at the **top-right rounded corner**, and that closing AppDock should **leave restored windows open**. The minimized-window symptom is specifically an existing tab whose selection leaves the app minimized.

## Implementation and reproduction

Startup previously skipped an entire app when discovery found multiple windows, or when any window was already attached. The updated resolver queues every eligible window, including minimized windows, skips occupied windows individually, and deduplicates window IDs. It still resolves only once, does not launch absent apps, and honors manual cancellation. Quiet startup restores only the first selection; remaining windows restore when selected.

Before the resolver change, `cargo test --all-features --locked startup_adds_all_eligible_windows -- --nocapture` failed in `startup_adds_all_eligible_windows_including_minimized_and_skips_only_occupied`: the first queue result was `None`, expected `Some(1)` (exit 101). After the change, it passes, including multiple windows, mixed eligibility, an initially minimized window, occupied windows, duplicate IDs, and notices for unavailable windows.

The top-right treatment already exists: the controls window disables its shadow while attached, and the backdrop matches the surrounding frame color. No further corner rendering change was made from the clarification alone. `keep_apps_open_on_close` already defaults to true; explicit Release continues to restore original minimized state. Existing explicit opt-outs are preserved.

## Native evidence and remaining symptom

[Native results](review-evidence/startup-all-windows-native/results.json): StartupMany, Startup, Minimized, RestoreReady, Animations, and A4 all passed. StartupMany uses three minimized child windows from one configured app, persisted config, and the real worker command queue. It verifies that all three tabs appear, that selecting each remaining tab reduces the minimized count, that selected windows have docked frames, and that closing leaves all three open. [Case log](review-evidence/startup-all-windows-native/StartupMany.log).

The sandboxed prerequisite attempt could not access Accessibility and is retained as a [prerequisite failure](review-evidence/startup-all-windows/results.json). The subsequent native run passed that prerequisite outside the sandbox. Only disposable children were controlled, with fresh `APPDOCK_DATA_DIR` directories and runner cleanup.

**Existing behavior protected:** a minimization event for a previously docked window pauses all docking. Selecting a tab then reports `Select Resume` and leaves windows untouched until Resume. `d2_minimizing_one_window_pauses_all_docking_until_full_resume` verifies this selection behavior. `quiet_startup_registers_all_tabs_but_only_restores_first_selected_window` also verifies that an observed, untouched minimized startup tab does not trigger that global pause.

**Validation pending:** WhatsApp/Spotify-specific selection failure remains unconfirmed by these fixtures. The earlier Spotify alternate-list discovery fix only establishes discovery and lifecycle membership; it does not establish successful restoration in those apps. Existing user windows were not manipulated, the installed AppDock bundle was not replaced, and the top-right corner was not rechecked on the user's app window. No claim of a provider-specific selection fix is made.

Implementation: working tree based on `f876eb2`; no commit created. Deterministic results and compiler checks are recorded alongside the native logs.
