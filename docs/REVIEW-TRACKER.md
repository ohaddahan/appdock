# AppDock review tracker

Reviewed implementation, 2026-09-10. Baseline `be198c9` had **37 passing tests**; the current working tree has **65 passing tests**. All entries below refer to this working tree. **Implementation commit: uncommitted, based on `be198c9`** (no implementation commit has been created). The original review reports and pre-existing `output/` remain untouched.

Dispositions distinguish **fixed**, **existing behavior protected**, **unconfirmed**, and **validation pending**. No D3/D6 production change preceded a failing reproduction. The version-1 workspace schema, dependencies, declared Rust minimum, and existing smoke command names remain compatible.

## A1 — Closed recovery blocks quit

- **Disposition: fixed.** Failed release retains lifecycle watching, but only live, previously docked attachments receive movement corrections. Restoration retries and polling share `MacBackend::refresh_window()`. Confirmed closure remains available after preflight until the engine removes the attachment, so later retry/poll calls still receive a typed closure result.
- **Reproduction:** `a1_failed_release_then_confirmed_close_allows_quit` failed before the fix; [baseline log](review-evidence/baseline-a1.log). Release failure followed by closure left a permanent recovery record.
- **Expected behavior:** successful restoration or positively confirmed closure clears recovery. A successful AXWindows enumeration or OS process-exit probe establishes closure; cached attributes, invalid handles, timeouts, permission loss, malformed enumeration, and ambiguous replacement identity cannot. A unique replacement is rebound without replacing the original restoration snapshot.
- **Regressions:** `a1_failed_release_then_confirmed_close_allows_quit`, `a1_failed_release_process_exit_and_transient_errors`, `a1_membership_ignores_cached_attributes_and_rebinds_only_unique_identity`, `a1_replacement_handle_restores_original_detached_snapshot`, `a1_confirmed_closure_survives_preflight_until_engine_acknowledgement`, `released_window_is_not_docked_again_after_restoration_failure`.
- **Validation:** deterministic tests pass. [Native A1](review-evidence/native-lifecycle-recheck/A1.log) passes failed restoration→close and failed restoration→process exit. An earlier harness failed to reap its child before the OS exit probe; [that failure is retained](review-evidence/native-final/A1.log). Replacement ambiguity and transient failures are simulated, not claims about every native app.
- **Implementation commit:** current uncommitted working tree on `be198c9`.

## A2 — Wrong highlighted tab

- **Disposition: fixed.** Confirmed selection controls the active underline. A disconnected action target has a separate left marker. Live action selection follows confirmed activation; a rename editor retains its original ID. Existing tab indicators update in place while editing.
- **Reproduction:** source inspection confirmed that preserved `editing` selected the active tab after a backend attachment selected another window. The Astra review also records its independent state reproduction.
- **Expected behavior:** attachment/replacement and successful direct activation highlight the confirmed window; failed attachment/switch does not highlight a requested window. Disconnected tabs remain actionable, including release. Snapshot updates do not retarget an open rename editor.
- **Regressions:** `a2_attach_replace_failed_switch_and_direct_activation_indications`, `a2_actions_follow_live_activation_and_preserve_disconnected_target`, `focus_failure_retains_previous_selection`, `disconnected_app_requires_replace_instead_of_duplicate_add`, all three existing `action_tests`.
- **Validation:** deterministic tests pass. [Native A2](review-evidence/native-verified/A2.log) checks actual TabButton state and different rendered underline pixels after selection changes. [Frame smoke](review-evidence/native-verified/frame.log) applies a changed snapshot through production selection synchronization while a real rename editor is open and checks the editor ID and rendered tab indicators. Rename and disconnected smoke modes pass. An initial bitmap check sampled the wrong image row; [its failure](review-evidence/native-final/A2.log) was a harness defect, corrected before passing native verification.
- **Implementation commit:** current uncommitted working tree on `be198c9`.

## A3 — Lost workspace geometry

- **Disposition: fixed.** The mailbox retains the latest geometry and its revision independently of queued movement commands. Processing a copied revision does not consume a later request. The worker also refreshes saved geometry after slow operations.
- **Reproduction:** source inspection confirmed Release/Pause/Quit removed queued Resize commands after the UI had already recorded that geometry as sent. The Astra review records an independent queue reproduction.
- **Expected behavior:** Release applies retained geometry to remaining attachments; detached handles stay untouched. Pause records without moving, Resume uses the latest area, and Quit saves without docking outgoing windows. Later geometry survives an older completion.
- **Regressions:** `a3_resize_release_multiple_releases_pause_resume_and_quit_save_latest`, `a3_old_geometry_acknowledgement_cannot_erase_concurrent_request`, `release_discards_stale_movement_switch_and_raise`, `newest_geometry_is_delivered_ahead_of_background_work`.
- **Validation:** deterministic tests pass, including reading the final saved JSON. [Native A3](review-evidence/native-verified/A3.log) exercises the actual worker mailbox with three child windows: queued resize, multiple releases, pause/resume, and resize→quit, then checks the persisted geometry. Existing frame and movement fixtures pass.
- **Implementation commit:** current uncommitted working tree on `be198c9`.

## A4 — Incomplete resume

- **Disposition: fixed.** Resume preflights all previously docked windows, restores their non-minimized frames to the current workspace, and focuses the prior selection last. Successful intermediate changes update expected state while original restoration snapshots remain intact. Any failure leaves docking paused.
- **Reproduction:** both inactive-window alignment and inactive-dialog preflight tests failed before the fix; [baseline log](review-evidence/baseline-a4.log).
- **Expected behavior:** inactive docked windows return after workspace movement/resize while paused, even without a selected tab. Never-docked attachments remain unmoved. Permission, dialog, fullscreen, and partial-operation failures remain retryable while paused; constrained native sizes are accepted.
- **Regressions:** `a4_resume_moves_all_previously_docked_windows`, `a4_resume_preflights_inactive_dialog_before_any_changes`, `a4_partial_resume_records_progress_and_can_retry`, `a4_no_selection_and_never_docked_attachments`, `a4_permission_and_fullscreen_failures_keep_global_pause`.
- **Validation:** deterministic tests pass. [Native A4](review-evidence/native-verified/A4.log) covers inactive docked windows, global minimization pause, moved/resized workspace, clamped size, and a never-docked attachment. Dialog/permission failure and partial-retry sequencing are controlled simulations.
- **Implementation commit:** current uncommitted working tree on `be198c9`.

## A5 — Late timeouts and blocking discovery

- **Disposition: fixed.** All native attribute/action operations use `Ax::request()` to prepare the exact object before the request. Discovery uses cancellation checkpoints before and after each request and publishes only completed scans. Pending discovery is coalesced. Priority commands interrupt scans; observation is deferred when a priority command is pending.
- **Reproduction:** source/API inspection confirmed the first AXSubrole read and replacement AXIdentifier reads preceded timeout initialization. [Apple documents](https://developer.apple.com/documentation/applicationservices/1459345-axuielementsetmessagingtimeout?changes=l_4) that equal objects do not share an object-specific timeout.
- **Expected behavior:** retain the one-second window-control timeout (and the existing shorter Dock badge timeout). Cancellation cannot interrupt an executing AX call; Release/Quit proceed once it returns, without finishing the scan. Cancelled scans preserve last completed picker results and retained attachments. Optional metadata reads propagate cancellation and permission errors.
- **Regressions:** `a5_every_request_prepares_its_object_before_reading`, `a5_cancellation_during_enumeration_stops_before_next_request`, `a5_discovery_coalesces_and_priority_interrupts_current_call`, A1 membership/rebinding tests, M2 AX error classification test.
- **Validation:** recorded deterministic call sequences cover app/window/replacement preparation, interruption, optional fallback, and priority ordering. [Native A5](review-evidence/native-verified/A5.log) cancels a real scan and verifies all attached handles remain usable. Native unresponsive-app timing and AX-object replacement remain broader compatibility gaps; the per-call bound is not a bound on an entire Release/Quit restoration sequence.
- **Implementation commit:** current uncommitted working tree on `be198c9`.

## A6 — Lost keyboard steps

- **Disposition: fixed.** Navigation resolves the latest valid requested ID in the current tab order, falling back to confirmed selection. Requests carry generations; acknowledging an older request cannot clear a newer one.
- **Reproduction:** source inspection confirmed each shortcut used the same confirmed snapshot, so two rapid next requests repeated the same target.
- **Expected behavior:** preserve each next/previous step through mixed directions and wraparound. Removed/reordered IDs, empty/single-tab workspaces, failed switches, and editing cancellation have defined fallbacks.
- **Regressions:** `a6_bursts_mixed_directions_wraparound_and_stale_acknowledgements`, `a6_reordered_removed_empty_single_and_invalid_requests`, `focus_failure_retains_previous_selection`, `editing_barrier_cancels_pending_focus_and_precedes_geometry`.
- **Validation:** deterministic tests pass. [Native A6](review-evidence/native-verified/A6.log) submits two unacknowledged next requests through the real worker and verifies selection reaches the third tab. OS hotkey conflict/delivery across all user apps remains outside this fixture.
- **Implementation commit:** current uncommitted working tree on `be198c9`.

## D1 — Timer intervals

- **Disposition: fixed.** Production publication/discovery use monotonic elapsed-time deadlines. Tick counts remain only for native fixture stages.
- **Reproduction/expected behavior:** tick ratios coupled production intervals to timer cadence. Preserve immediate initial refresh, 2.1-second publication, and 10.5-second discovery. Delayed callbacks trigger once, without catch-up bursts; pointer suppression leaves discovery due until the pointer is released.
- **Regressions:** `d1_initial_boundaries_delays_and_pointer_resumption` uses a fake clock for exact boundary, delay, pointer, and resumption cases.
- **Validation:** deterministic test and native frame/picker/tracking fixtures pass.
- **Implementation commit:** current uncommitted working tree on `be198c9`.

## D2 — Global pause

- **Disposition: existing behavior protected; per-window-pause proposal declined by explicit product choice.** Minimizing one previously docked window continues to pause all docking.
- **Reproduction/expected behavior:** this is intended behavior, not a defect. A4 must resume all previously docked windows correctly.
- **Regressions:** `d2_minimizing_one_window_pauses_all_docking_until_full_resume`, A4 regression group.
- **Validation:** deterministic test and native A4 pass.
- **Implementation commit:** current uncommitted working tree on `be198c9` (regression coverage; global policy retained).

## D3 — Frame settling

- **Disposition: fixed, confirmed by scripted and native failing reproductions.** Require two consecutive stable polling intervals. Four 40 ms waits retain the 160 ms sampling budget; IPC time is additional. Absence of stability returns typed `UnsettledGeometry`.
- **Reproduction:** [scripted delayed/oscillation failures](review-evidence/baseline-d3-scripted.log). The first native attempt used an AX setter hook AppKit did not invoke, so [its passing result](review-evidence/native-baseline/D3.log) did **not** validate delayed behavior. The corrected disposable NSWindow frame hook recorded a change after the first poll: [native failure](review-evidence/native-delay-reproduction/D3.log), returning the old frame at 45 ms before a queued 65 ms change. Production settling was changed only after this evidence.
- **Expected behavior:** delayed/clamped geometry settles before success; oscillation fails and restoration retains its snapshot for retry. This finite sampling rule does not promise detection of arbitrarily late changes.
- **Regressions:** `d3_delayed_geometry_must_not_succeed_on_first_unchanged_poll`, `d3_oscillation_reports_unsettled_geometry`, `d3_clamped_frame_is_accepted_after_two_stable_intervals`, `d3_restoration_can_retry_after_unsettled_geometry`, `d3_unsettled_release_retains_snapshot_until_successful_retry`.
- **Validation:** deterministic tests pass. [Native D3](review-evidence/native-verified/D3.log) records delayed target callbacks and returns the final requested frame (174 ms total including IPC). Existing constrained-restoration and frame smoke fixtures pass. Native oscillating targets are not claimed; oscillation coverage is scripted.
- **Implementation commit:** current uncommitted working tree on `be198c9`.

## D4 — Focus checks

- **Disposition: existing behavior protected; removing checks declined on evidence.** Retain checks before each focus operation and during polling. A queued-command barrier cannot protect operations already executing. Cancellation also prevents confirming a focus operation interrupted by editing.
- **Reproduction/expected behavior:** the controlled adapter suspends editing before/between frontmost, raise, main-window operations and during polling; no subsequent focus operation may run. The requested removal would fail these cases.
- **Regressions:** `d4_editing_between_native_operations_and_during_polling_stops_focus`, `editing_barrier_cancels_pending_focus_and_precedes_geometry`.
- **Validation:** deterministic tests pass. Native frame smoke verifies picker/rename keyboard ownership and queued Raise behavior. The final full serial run had [one setup focus timeout](review-evidence/native-verified/D4.log), before its cancellation assertion. The unchanged fixture [passed in isolation](review-evidence/native-focus-recheck/D4.log). Earlier runs also passed. **Remaining native gap:** occasional focus acquisition during rapid fixture startup was observed and is not claimed resolved.
- **Implementation commit:** current uncommitted working tree on `be198c9` (adapter and coverage; checks retained).

## D5 — App-list encapsulation

- **Disposition: fixed.** `MacBackend::apps` is private; worker and diagnostics use `update_apps()`.
- **Reproduction/expected behavior:** direct writes bypassed a single update boundary. Discovery follows additions/removals while lifecycle checks do not infer process death from a stale app-list snapshot.
- **Regressions:** A1 membership/lifecycle regressions and native D5.
- **Validation:** compilation enforces encapsulation. [Native D5](review-evidence/native-verified/D5.log) removes/re-adds the app list, checks discovery results, and verifies retained live handles throughout.
- **Implementation commit:** current uncommitted working tree on `be198c9`.

## D6 — Backdrop lifetime

- **Disposition: fixed, confirmed by a native failing reproduction.** `Backdrop` clones share an `Rc<Inner>`; only final inner destruction orders the native window out.
- **Reproduction:** [native baseline](review-evidence/native-baseline/D6.log) recorded `final_owner_visible=true` after dropping both wrappers. Observation used WindowServer IDs and did not retain the tested NSWindow.
- **Expected behavior:** dropping one clone preserves visibility; dropping the final owner removes it. Never hide on every wrapper's Drop.
- **Regressions:** native `--review-fixture D6`; existing `cover_includes_larger_inactive_windows_and_negative_desktop_coordinates` protects coverage geometry.
- **Validation:** [native D6](review-evidence/native-verified/D6.log) records `final_owner_visible=false`; intermediate clone drop stayed visible. Existing overlay/restoration/frame fixtures pass.
- **Implementation commit:** current uncommitted working tree on `be198c9`.

## M1 — Extract native smoke machinery

- **Disposition: fixed.** Native fixture state, stage execution, and verification helpers moved into `src/ui/fixtures.rs`; review-only targets/harness live in `src/ui/fixtures_review.rs`. RefCell borrows are still released before reentrant AppKit focus/rename/picker/window calls.
- **Expected behavior:** preserve existing smoke commands and add issue-selectable `--review-fixture`. The serial runner owns its children, kills only each case's process group, uses fresh temporary APPDOCK_DATA_DIR, and records environment, outcomes, durations, exit codes, and logs. A supplied existing workspace is rejected.
- **Regressions/validation:** native movement, restoration, frame, pointer, rename, disconnected, and tracking fixtures pass. [Isolation guard](review-evidence/isolation-guard.log) verifies an existing workspace remains byte-for-byte unchanged. [Sandbox prerequisite evidence](review-evidence/sandbox-prerequisite/results.json) reports denied Accessibility as prerequisite failure (exit 2), never pass.
- **Implementation commit:** current uncommitted working tree on `be198c9`.

## M2 — Typed backend errors

- **Disposition: fixed.** Backend errors distinguish confirmed closure, permission loss, communication failure, cancellation, unsettled geometry, and other failures. AX codes, operation context, and aggregate/rollback causes survive until presentation.
- **Expected behavior:** an invalid AX handle or failed request alone never means closed. User-facing text is produced through Display at status/alert/diagnostic boundaries.
- **Regressions:** `m2_ax_failures_preserve_codes_and_never_establish_closure`, A1 transient-failure tests, A5 cancellation tests, D3 unsettled-restoration tests.
- **Validation:** all deterministic tests and native review cases above; public workspace JSON unchanged.
- **Implementation commit:** current uncommitted working tree on `be198c9`.

## M3 — Hosted macOS CI

- **Disposition: fixed configuration; hosted execution validation pending.** Added pull-request/push workflow using explicit `macos-15`, listed in [GitHub's supported runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
- **Expected behavior:** hosted deterministic fmt/test/Clippy/locked build, plus an all-target/all-feature check on the declared Rust 1.95 minimum. Native fixtures run locally where Accessibility and a visible desktop are available.
- **Validation:** local `cargo fmt --check`, `cargo test --all-features --locked` (65/65), `cargo clippy --all-targets --all-features --locked -- -D warnings`, `cargo build --all-features --locked`, and `cargo +1.95.0 check --all-targets --all-features --locked` pass. Logs: [tests](review-evidence/deterministic.log), [Clippy](review-evidence/clippy.log), [build](review-evidence/build.log), [minimum Rust](review-evidence/msrv.log). `git diff --check` passes. **Hosted CI has not run.**
- **Implementation commit:** current uncommitted working tree on `be198c9`.

## Native scope and remaining gaps

[Final full-run manifest](review-evidence/native-verified/results.json): macOS 26.6.2 (25G83), Apple Silicon, Rust 1.97.1, 17 serial cases; 16 passed, D4 had a setup timeout. [D4 isolated rerun](review-evidence/native-focus-recheck/results.json) passed. After the final closure-retention correction, [all five affected lifecycle/geometry/resume/discovery cases passed again](review-evidence/native-lifecycle-recheck/results.json). Prior failed attempts remain available and are explained above. The normal sandbox denied Accessibility; the authorized native launch context reported trusted access.

No existing user application windows were attached or changed. No installation, packaging, deployment, permissions changes, or hosted workflow execution occurred. Native error/permission revocation, AX-object replacement across diverse apps, focus acquisition under competing desktop activity, Intel/other macOS releases, multiple displays/Spaces, and long-running use remain outside the demonstrated native coverage. Controlled deterministic cases cover the specified failure paths, not universal desktop behavior.
