# Release workflow fix — 2026-09-10

[Run 34497705015](https://github.com/ohaddahan/appdock/actions/runs/34497705015/job/102940280896) failed in checkout before Rust ran: it tried to fetch the nonexistent `refs/tags/v0.1.0` supplied as a manual input. [Failure excerpt](review-evidence/release-workflow/failure.log).

The release workflow now has **only `workflow_dispatch`**, with no version/tag input. It checks out the selected immutable revision, reads `[package].version` from Cargo.toml, and derives its tag. Missing tags are created during a manual run; existing tags must point to the same commit. Both macOS architectures build that exact SHA. Release assets are published only after both builds succeed. Pushes, tag pushes, and release-publication events do not start this workflow.

Both workflows use `Swatinem/rust-cache@v2` after installing their toolchains. Release keys separate native architecture/runner; check keys separate OS/architecture. The action includes compiler and Cargo dependency state automatically. Deployment target and SDK environment are included, and cached tool binaries are disabled. Manual release runs on the same branch can reuse their caches. No automatic release/cache-warming runs were added. [Cache action reference](https://github.com/Swatinem/rust-cache).

Validation: actionlint, YAML parsing, shell syntax, and **8 release resolver scenarios pass**, including absent tags, annotated existing tags, wrong-commit collisions, prereleases, and rejecting non-manual events. Tag creation is mocked in tests; no remote refs or releases were created. [Resolver tests](review-evidence/release-workflow/resolver-tests.log).

The matching local release command (`RUSTUP_TOOLCHAIN=1.95.0 MACOSX_DEPLOYMENT_TARGET=12.0 CARGO_INCREMENTAL=0 cargo build --release --locked`) succeeded in 18.64 seconds initially and 0.04 seconds on an identical repeat with no recompilation. [First run](review-evidence/release-workflow/release-first.log), [warm run](review-evidence/release-workflow/release-warm.log). These are local Cargo reuse measurements; hosted GitHub cache hits and the corrected workflow still require a fresh run after pushing the changes. Re-running the old failed run uses its old workflow definition.

## Developer ID signing and notarization

Manual releases require these repository Actions secrets:

- `APPLE_CERTIFICATE_P12_BASE64`: base64-encoded Developer ID Application certificate and private key in a password-protected PKCS#12 file.
- `APPLE_CERTIFICATE_PASSWORD`: the PKCS#12 export password.
- `APPLE_APP_SPECIFIC_PASSWORD`: the Apple Account app-specific password for notarization.
- `APPLE_ID`: that Apple Account email.
- `APPLE_TEAM_ID`: the developer team that issued the certificate.

After packaging, each macOS job imports the certificate into a temporary keychain,
signs the AppDock executable and bundle with hardened runtime and secure timestamps,
and submits the app to Apple. Only Accepted submissions continue. The approval
ticket is stapled and validated, and Gatekeeper assessment must pass before the
final ZIP and checksum are created. Either architecture failing blocks publication.
Temporary signing credentials are removed on success or failure. The build job
has a 60-minute limit, including a 30-minute signing step with a 20-minute notary
wait. Local `scripts/package.sh` output remains ad-hoc signed.

[Apple notarization documentation](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow)
and [GitHub certificate setup](https://docs.github.com/en/actions/how-tos/deploy/deploy-to-third-party-platforms/sign-xcode-applications).
