# Confirmed ngit bridge updates

**Goal:** Owner-only `/update` discovers stable codr releases through ngit and
offers a ten-minute, chat-bound confirmation before updating only the worker.

**Architecture:** Embed a small Python updater and the existing installer/service
helpers in the binary. Rust owns command routing, durable confirmation and job
receipts. An independent systemd/launchd job downloads verified Blossom bytes,
stops only the managed bridge, keeps a stopped-state and executable backup,
atomically replaces the executable, and checks the new worker's startup receipt.
It restores the old executable after failed startup, never an older state file.

**Trust:** Pin codr's existing repository/application author. Use ngit v3 signed
release/asset records, channel `main`, exact numeric versions and platform asset
names. Pin the release event and asset hashes in the offer and revalidate on
confirmation. No GitHub queries or download fallback; reject GitHub URLs and
redirects. Require both archive and checksum records. Reject changed connector
pins, protocol or state cohort through the candidate's read-only `update-info`.
No signing credentials, connector upgrades or storage migrations are needed.

## Implementation checklist

- [x] `tests/test_update.py`: fixture signed-record shapes; reject wrong author,
  preview, mismatched version/platform, bad hashes, GitHub redirects, changed
  release records and incompatible cohorts. Test managed-service launch,
  successful installation, failed download, failed health, and rollback failure
  without live network or service operations.
- [x] `scripts/install.py`: add optional checksum URL, pinned archive hash and
  download function to reuse safe extraction and macOS signature checks.
- [x] `scripts/update.py`: ngit discovery, independent service-manager job,
  exclusive update lock, staged validation, stop/backup/replace/start/health,
  and atomic result receipt. Bound subprocesses and download sizes.
- [x] `tests/update.rs`: persist and reload offers, wrong/expired/cross-chat
  confirmations, cancel, duplicate inputs, global in-progress protection,
  startup/result receipts and failed-save rollback. Exercise owner-only routing
  with no network or real service-manager actions.
- [x] `src/update.rs`, `src/state.rs`, `src/lib.rs`, `src/main.rs`,
  `src/bridge.rs`: integrate commands before project menus, embed helpers, add
  `update-info`, persist one global installation intent before launch, and report
  completion through the original encrypted chat. Never retry uncertain launches.
- [x] `README.md`, `docs/REFERENCE.md`: document `/update`, confirmation/cancel,
  ngit v3 prerequisite, supported managed installs, worker-only scope and recovery.
- [x] Verify the red tests first, then focused Python/Rust tests, full existing
  suites, `cargo fmt --all --check`, strict clippy and `git diff --check`.
- [x] Obtain two independent read-only reviews; resolve demonstrated findings
  and verify again. Do not install a test update or move a release tag for this
  feature. Preserve the separate in-flight 0.7.0 release and existing account data.

## Verification record

- 321 Rust tests passed; 8 existing tests ignored. 110 Python tests passed.
- Formatting and strict all-target clippy passed.
- Both independent reviews passed after fixing managed-PID verification on macOS
  and canonical configuration-path comparison, each with a failing-first test.
- Native service-manager/rollback actions use fixtures in tests. No actual
  release update, account migration or test release publication was performed.
- Contributor-credit policy now requires both GitHub and ngit contributors in
  published and mirrored notes; it is recorded in shared/local instructions,
  the starter template and `docs/MIRRORING.md`.
