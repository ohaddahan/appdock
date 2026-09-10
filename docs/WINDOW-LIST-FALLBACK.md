# Window-list fallback — 2026-09-10

The screenshot showed minimized Chrome windows in the picker, so the remaining report required app-specific inspection. A live, read-only Spotify check found **AXWindows empty while AXChildren contained an AXWindow**. The old backend used only AXWindows for discovery and lifecycle membership, which could omit a window or incorrectly classify its absence as closure.

The backend now merges both public lists, filters child elements by AXWindow role, and deduplicates by canonical AX equality. Discovery and lifecycle polling use the same merged list. Errors and unavailable lists are not converted into positive closure evidence. No title-based matching or private window APIs were added.

The updated production backend found **one eligible Spotify window and confirmed its lifecycle membership**. [Live diagnostic](review-evidence/window-list-fallback/live-discovery.log). That observation was made while the Spotify window was not minimized; it proves the alternate-list discovery fix, not universal minimized behavior. No matching WhatsApp process was found in the accompanying inspection. Existing app windows were not moved, focused, minimized, or attached during the live check. Apps already attached to tabs remain intentionally excluded from the Add App picker.

**83 tests pass**, plus strict Clippy and Rust 1.95 checks. [Tests](review-evidence/window-list-fallback/tests.log), [Clippy](review-evidence/window-list-fallback/clippy.log), [Rust 1.95](review-evidence/window-list-fallback/msrv.log). The disposable A1 lifecycle, A5 cancellation, and Minimized fixtures also pass: [native manifest](review-evidence/window-list-fallback/results.json).

For future read-only diagnosis without window titles or chat content:

```sh
cargo run --locked -- --diagnose-windows Spotify WhatsApp
```
