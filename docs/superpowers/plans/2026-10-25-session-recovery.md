# Session recovery and replacement protection

## Approved scope

Restore original native sessions and existing phone-group mappings, preserve replacement conversations, and prevent temporary OpenCode failures from silently creating replacement chats. Do not migrate account storage, delete history, or publish a release.

## Recovery

- [x] Stop and verify the bridge worker.
- [x] Save a private, device-local copy of current routing, forwarding, configuration, and Herdr layout.
- [x] Query the connector for every retained group; all 22 groups remain available.
- [x] Restore original session IDs from the pre-incident terminal mappings. Use the session picker only when the selection is unambiguous; otherwise restart only the TUI with its exact original session ID and an explicit existing server connection. Preserve the backend sessions. The recovery conversation's frontend was restarted last; its backend turn was not interrupted.
- [x] Verify live terminal-to-session IDs and unchanged original group IDs before resuming.

## codr safeguard

In `src/bridge.rs`, detect a previously unseen session ID on a terminal with a retained group before reading history or creating a group. Persist a pending mapping with `session_change_from` in `src/state.rs`; exclude that candidate from agent routing and keep the old mapping paused rather than ended. A local `confirm-session-change KEY` command in `src/main.rs` must verify a fresh native snapshot against the recorded candidate before clearing the hold. The command uses the same exclusive state lock as existing repair commands. It never moves the old chat to the new session. Existing mapped sessions and first sessions in new terminals remain supported; confirmed `/new` and `/agent` launches remain intentional creation paths.

- [x] Add regression tests to `tests/bridge.rs` for replacement detection, restart persistence, safe return to the original session, fresh confirmation, stale confirmation, inbound/queued delivery isolation, and intentional new-terminal launches.
- [x] Run the new tests before implementation and observe the missing guard.
- [x] Implement the guard and confirmation command; document recovery in `docs/REFERENCE.md` and the unreleased changelog.
- [x] Run `cargo fmt --check`, targeted tests, and the Rust test suite through the build-storage adapters.

## OpenCode safeguard

Work separately in `/home/tom/code/opencode` on the upstream `dev` base, retaining upstream contribution rules. In `packages/tui/src/routes/session/index.tsx`, preserve the selected session on transient synchronization failures, show an error, and retry on reconnect or explicit user action. Keep confirmed deletion handling separate. Do not replace the live shared server for testing.

- [x] Add fixture-driven reconnect regression tests before changing the production effect.
- [x] Verify temporary failure retains route, history and tab; recovery uses the original ID; deletion and deliberate new-session commands still work.
- [x] Run the relevant TUI tests and type checks. Report source verification separately from any installed-client change.

## Delivery gates

- [x] Request two independent read-only reviews in parallel: correctness/contracts/security, and tests/regressions/scope.
- [x] Fix demonstrated findings and rerun relevant checks. A duplicate-snapshot bypass was reproduced, then fixed by validating all native identities before reconciliation side effects. Both snapshot orders are tested; both reviewers cleared the final changes.
- [x] Install only verified local changes needed for recovery. Verify the original groups and native identities again before starting the bridge. Keep replacement histories and mappings unchanged.
- [x] Report recovery, installed safeguards, source-only safeguards, and any blocker separately. No commits, pushes, tags, or releases without authorization.

## Verified outcome

- All 19 original native sessions are selected and all 19 original chat mappings are active.
- All 22 retained groups, including the replacement chats, remain available under their unchanged group IDs. No conversation was deleted or merged.
- The reviewed local bridge worker is installed and running. The previous forwarding-off preference is retained; a normal authorized phone wake can resume forwarding.
- OpenCode `2.1.4-session-recovery.1` is installed locally and all 19 terminal clients run that artifact. The complete native artifact was built once with Bun 1.4.4 and the official single-platform build script, including web UI assets. The original shared server process was not restarted.
- The complete Rust suite passed. The relevant OpenCode suite passed 75 tests, including retry without reconnect and confirmed missing-session handling. `tsc -b`, targeted `oxlint`, formatting and diff checks passed. The default `bun check` script was unavailable in the system Bun 1.4.2; the repository's supported TypeScript fallback passed.
- History-reader checks passed for all 19 original sessions after installation. No phone-device round trip was performed, and the cause of the original shared-service stall remains unproven.
- Executables, configuration, current state, checksums, native recovery manifests, and installation evidence are retained in a private device-local recovery backup outside Git. Source changes remain uncommitted and unpublished in both checkouts. No release or mirror publication was attempted by this recovery task.
