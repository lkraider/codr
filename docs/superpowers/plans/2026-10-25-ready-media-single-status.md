# Ready media and one active status

**Contract:** Image captions and voice transcripts are submitted as soon as
download/transcription completes, including while the agent works. Preparation
is asynchronous and durable, not a scheduler for future agent tasks. Do not
interrupt the agent, lose accepted work, reset an existing response or uncertain
submission, or interpret media captions as bridge commands. Native blocking
dialogs, unavailable sessions and proven transport failures still defer safely.

Reuse the existing media FIFO and prepared snapshots. Media can pass older
legacy text deliveries just as new text can; image ordering remains FIFO. Keep
legacy text/portr queue policy unchanged. Forced sync-off controls outgoing
forwarding, not acceptance of prepared owner media.

Reuse the active editable progress message for follow-ups and blocked-to-working
transitions. Do not send extra standalone working notices for those events or
for accepting media. Preserve existing history and compatibility with pending
legacy notices; do not delete old chat messages.

- [x] Add failing busy-image, busy-audio and nonduplicating-status regressions.
- [x] Remove only the native-idle gates for media preparation and prepared input;
  preserve identity checks, deduplication, receipt durability and known-unsent retry.
- [x] Update old tests whose idle/extra-status expectations were intentionally
  superseded; retain restart, FIFO, limits, mute, blocked-dialog and failure coverage.
- [x] Update reference/help text so preparation is not described as agent task queuing.
- [x] Run full Rust/Python tests, formatting, strict clippy and build smoke tests.
- [x] Obtain two independent reviews and resolve demonstrated findings. Preserve
  the separately implemented, uncommitted ngit updater and release state.

## Verification

328 Rust tests passed (8 existing ignored), 110 Python tests passed, strict
clippy and formatting passed. Both independent reviews passed after correcting
the reference's remaining legacy-idle wording. Coverage includes known-unsent
prepared-image requeue at its original slot, preserved older uncertain flights,
uncertain submission across restart, failed-save retention, changed native
identity, voice text resembling a bridge command, and ready media passing an
unrelated download. Old unsent standalone refreshes are discarded; historical
chat messages are never deleted. The local worker has not been replaced by
this feature build.

## Project-name confirmation follow-up

The user also requires final project setup confirmation to use the repository
folder basename, not the workspace destination label. The destination remains
visible and must still be revalidated. This applies to saved requests, existing
workspace tabs, nested/dotted folders and the one-line launch command. Keep
`0` as cancellation; a folder named `0` uses explicit `/new confirm`.

- [x] Observe failing existing-workspace and dotted-folder confirmation tests.
- [x] Derive confirmation from the existing request's directory, with no new state
  fields or migration, and update the wrong-name message and reference text.
- [x] Verify launch-flow tests including restart, rename, expiry, owner checks,
  duplicate confirmations and zero-cancel safety.
- [x] Run combined verification and obtain both independent final reviews.

Combined verification: 331 Rust tests passed (8 ignored), 110 Python tests
passed, strict clippy, formatting and diff checks passed. Both independent
reviews reported no findings. The optimized build and empty-HOME `update-info`
smoke test passed; this build is not installed or published. A subsequent full
Rust run encountered a transient failure in the existing phone-echo reconciliation
test; that test passed alone and the next full run passed without code changes.
Logs: `/tmp/opencode/codr-phone-echo-recheck.log` and
`/tmp/opencode/codr-combined-final-recheck.log`.

## OpenCode session confirmation follow-up

The reported `lockr` launch created its folder and native session, but stopped
before tab/agent creation. Read-only API inspection confirmed the recorded
session's exact ID, directory and requested model/provider, with OpenCode's
added `variant: "default"`. Exact JSON comparison incorrectly treated this as
an unconfirmed creation. In-place model switches used the same comparison.

The shared model readback matcher now permits only omitted requested variant
to literal `default` normalization. Explicit variants, provider/model, session
ID and directory remain checked. No live launch record, folder, session or
service was changed; earlier uncertain launches are not automatically retried.

Creation and switch regressions failed before implementation. Final verification:
334 Rust tests passed (8 ignored), 110 Python tests passed, strict clippy,
formatting and diff checks passed. Two independent read-only reviews found no
issues. Optimized build passed; not installed or published. Evidence:
`/tmp/opencode/codr-canonical-variant-{rust,python,clippy,build}.log`.

## Orphaned status completion follow-up

The user authorized repair of standalone working statuses left after completed
requests. Eight inspected messages in the current codr chat predated the
single-status installation. Each was confirmed as this account's available,
untruncated, exact `> working...` text, strictly timestamp-older than a confirmed
done status, and not a current outbox edit target. They were changed to `> done`
and verified through local timeline readback. No answers were edited. Phone
receipt was not independently verified. Private repair evidence is under
`~/.local/state/herdr-white-noise-bridge/orphan-status-repair-20261025T021824Z/`.

Further production fixes retain known completion targets after failed edits
without blocking final answers. Mute/resume preserves attempted progress and
durable completion evidence if its final is observed before runtime idle. An
early idle alone still does not finish progress. Unknown initial-send targets
are retained for inspection and reported by `status` as `unresolved_progress`,
not replayed after completion: the connector's deduplication receipts are
bounded and cannot guarantee indefinite same-message recovery.

Final verification: 338 Rust tests passed (8 ignored), 110 Python tests passed,
strict clippy, formatting and diff checks passed; both read-only reviews passed
after addressing their findings. The optimized worker was installed with all
20 mappings, configuration, account, queues and submission receipts preserved;
the connector remained unchanged. Installed SHA-256:
`7412b0f44d2e5aa4a4cdd146666876c8d82d6749f193d8faf230701a3ee685cb`.
Backup: `worker-reviewed-fixes-backup-20261025T021821Z` beside routing state.
This is a local build, not the published v0.7.0 artifact; no tags were changed.

## Release preparation and attribution

The user selected 0.8.0 for the later features and fixes and requested explicit
credit for Paul Eipper (`lkraider`). GitHub's commit API identifies the original
Claude-reader commit `1b7d6da9faacb6ca976d0275dce5318bdf184e0d` as authored by
`lkraider`; original author records and the ngit integration commits confirm
the contribution scope. Juraj Bednar's `jooray` credit and PR #6 are retained.

Cargo, lockfile and plugin versions are prepared locally as 0.8.0. Rust updater
fixtures now offer 0.8.1 so the version comparison remains meaningful. The full
338 Rust tests (8 ignored), 110 Python tests, clippy, formatting and diff checks
passed. The server restart interrupted tool reporting, not the successful
test/clippy processes; completed logs were inspected rather than rerun.
Both independent release-scope/attribution reviews reported no findings.

Published GitHub v0.7.0 notes now match `docs/releases/0.7.0.md`, including the
full feature groups, upgrade cautions, known issues and Paul/Juraj/Tom credits.
All 16 release asset records and the release timestamp were verified unchanged.
The published tag was not moved. New `docs/releases/0.8.0.md` and the changelog
clearly mark 0.8.0 as unreleased and distinguish prior contributor work.

No 0.8.0 source commit, push, tag, public build or release has been made. The
live worker is unchanged from the recorded local 0.7.0 build. The v0.7.0
release-asset mirror and identical mirrored notes remain pending verification;
no replacement ngit release or signing request was created during this task.

## Generic file-attachment follow-up

The user reported `white-noise-diagnostic-logs.zip` failing with an unrelated
media/transcription error. The bridge rejected all non-image/audio MIME types
before download and skipped reference hydration for those files. ZIP preparation
and ZIP offline-reference regressions failed before the implementation change.

Non-audio files now share the private-copy path: UUID destination names, 0700
directory, 0600 regular files, bounded 25 MiB reads, matching download MIME/size,
no-follow opens and inode checks, and a quoted original filename alongside the
caption and path. Archives are never extracted or executed by the bridge.
Reference hydration also handles generic files. File preparation errors no
longer point to the transcription service; preparation uses a plain acceptance
notice rather than an orphaned downloading status. Image and voice behavior is
preserved.

Coverage includes ZIP, PDF, text, opaque binary and SVG-as-file delivery;
traversal/control-character filenames; size/MIME/symlink rejection; offline
references; restart after removing the original download; delivery while
working/sync-off; duplicate prevention; and preservation of an older uncertain
flight. All 342 Rust tests passed (8 ignored), 110 Python tests passed, and
clippy/formatting/diff checks passed. Both independent read-only reviews found
no demonstrated defects.

The reviewed local 0.8.0 worker is installed (PID 3529357), SHA-256
`8a5df5e19e812eb7d6ea893433686df44fb9be82fbd11097fce74921e71a1352`.
All 20 mappings, account/configuration, accepted queues and submission receipts
were preserved, and the connector was unchanged. Backup:
`worker-reviewed-fixes-backup-20261025T084952Z` beside routing state.
The earlier rejected ZIP messages were not replayed; the owner must resend.
0.8.0 remains uncommitted/unpublished; no tag or public build was started.
