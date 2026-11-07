# 0.8.0 release preparation

## Scope and publication boundary

The user requested preparation of codr 0.8.0. Prepare a reviewed local release
branch and source commit. Do not push, merge, tag, dispatch CI, create a release,
change the running worker or publish signed mirror records during preparation.
Do not move published v0.7.0 or mix `dev/linux` into stable.

Included: confirmed ngit/Blossom `/update`, immediate prepared-media input,
generic file attachments, repository-folder confirmation, the OpenCode canonical
default-variant fix, concise welcomes, retained progress completion targets,
request-bound custom text replies for OpenCode V2 questions, and matching
documentation/tests. Preserve user README edits and every screenshot.

Juraj Bednar's open GitHub PR #7, standalone control chat, is not merged and is
not included. Its successful checks do not verify this release candidate.

## Artifact inspection and reuse

- GitHub master and published v0.7.0 resolve to
  `f45578611629069f0ef9cd50b59e4994e07d018b`.
- The successful four-platform 0.7.0 run is `40146376024`. Its binaries have the
  wrong source/version for 0.8.0 and cannot be relabelled.
- PR #7 checks run `40177278668` has no retained artifacts and tests different
  source. There is no existing 0.8.0 tag, candidate branch or build to promote.
- Full manual dispatch with `linux_preview=false` builds all four platforms but
  does not publish. A v-tag push builds again and publishes. There is no approved
  artifact-promotion path. Avoid a redundant full manual build before a tag
  merely to repeat compilation; explain the tag-triggered build before starting it.
- `linux_preview=true` publishes a Linux prerelease and can create its tag. It is
  not a build-only check and is not appropriate for this full release.

The workflow's notes-only change checks out the exact tagged source and uses
`docs/releases/0.8.0.md`, rather than GitHub's incomplete generated notes. All
existing checks, platform builds, connector tests and publication conditions
remain in place. This is not an artifact-promotion workflow change.

## Preparation checklist

- [x] Inspect refs, open PRs, workflow triggers, runs and retained artifacts.
- [x] Keep package, lockfile and plugin version at 0.8.0.
- [x] Prepare publishable grouped notes, credits and versioned install examples.
- [x] Add regression checks for version/notes/install consistency and reviewed
  notes publication without changing release gates.
- [x] Verify the combined source and obtain two independent read-only reviews.
- [x] Prepare the reviewed candidate on local `release/0.8.0` for its source commit.

The prepared source candidate is the commit containing this record on
`release/0.8.0`; resolve its exact SHA with Git rather than a self-referential
hash in the committed file. Publication remains outside this preparation task.

## Verification record

The full candidate passed 348 Rust tests (8 ignored), 113 Python tests, strict
clippy, formatting and diff checks. The optimized local build and empty-HOME
`update-info` smoke test passed, creating no account or configuration state.
The candidate binary SHA-256 is
`411ecbd1664fdf1643bf2e8034c85f771ec8bf62ba5d0097c26bda55cfc37011`;
this local executable is not a published or four-platform release artifact.
Logs are `/tmp/opencode/codr-v080-candidate-*-final.log`.

Both independent reviews reported no remaining findings after corrections.
Native question codes now bind the tool identity, native form and full question
content. A failing-first regression proves that an old coded answer cannot
reach a replacement form even after its new card is announced. Plain and coded
custom answers use the authenticated form API only, with intent saved before
submission; duplicates and uncertain results never automatically resubmit.
Prepared media is not interpreted as a custom question answer. The live native
question has not been answered, cancelled or otherwise changed during this work.

The existing live worker is unchanged from the local attachment-fix build,
SHA-256 `8a5df5e19e812eb7d6ea893433686df44fb9be82fbd11097fce74921e71a1352`.
It does not yet include custom text question replies. No public CI run, tag,
release or mirror signing request was triggered by candidate preparation.

## Gates before an authorized public release

1. Confirm the reviewed candidate SHA and any later changes. Set the changelog
   release date at publication; do not claim the release is already public.
2. Publish approved source to GitHub first, then mirror that exact ref to ngit
   with Amber approval and verify matching hashes. Do not force-push or merge
   stable/preview branches without authorization.
3. Push a new v0.8.0 tag only when authorized. This necessarily triggers the
   version-specific four-platform build and connector gates; do not replace or
   reuse the published v0.7.0 tag/assets.
4. Verify all four targets and the exact published archives/checksums. Green
   source checks or locally installed 0.8.0 are not a published release.
5. Mirror the exact GitHub bytes and identical notes, contributor credits,
   source commit, date and stable channel to ngit/Blossom. Verify every downloaded
   mirror hash. Check existing records before retries and stop on conflicts.
6. Report source, tag, CI artifacts, release, installation and mirror states
   separately. Existing v0.7.0 release-asset mirroring remains pending separately.

No real `/update` replacement has been exercised. Scheduling and rollback tests
use fixtures. A suitable published compatible release and explicit confirmation
are required for that end-to-end test. No connector/storage migration is needed
or authorized; preserve the current MDK 0.14 cohort and all user data.

## Outstanding-PR integration update

The earlier verification and installation records above describe the initial
candidate. This section records the later, authorised PR-integration task.

- GitHub PRs #8–#15 were reviewed and merged, retaining Juraj Bednar's original
  commits. Fresh ngit discovery found no open or draft proposals.
- Integrated `master` is `11b316a7b23e1ada306f1fbf27fc18c8b070b9be` on GitHub
  and ngit. GitHub checks passed in run `40194163306`.
- Review fixes preserve prepared/accepted input before command expansion, use
  a validated directory handle for folder listing, retain legacy `list`/`pick`
  workspace labels, distinguish pending from delivered blocked notices, and
  avoid consuming blocked announcements while muted. Failing-first regressions
  cover these paths. README wording preserves ended-chat history.
- The session-replacement safeguard is committed on the release branch. The
  locally installed OpenCode recovery build remains separate from codr source
  and release assets. No live services or account data changed in this task.
- Candidate integration keeps repository-folder-name confirmation alongside
  `confirm`/`yes`/`y`, retains `/update` help, and treats `/u` as ambiguous between
  `/unmute` and `/update`. Retained editable progress is not cancelled by the
  blocked-notice fix. The updater-prefix regression failed before correction.
- Final local candidate checks: 370 Rust tests passed, 8 ignored, and 122 Python
  tests passed. Formatting and strict clippy passed. Logs use the prefix
  `/tmp/opencode/codr-v080-pr-batch-`.
- Release notes cover the new PR features, session safeguard and review fixes,
  with Tom Dwyer, Paul Eipper/lkraider and Juraj Bednar/jooray credit. Screenshots
  remain unchanged. The changelog remains marked unreleased.

Release preparation permits build-only verification, not a version tag, release,
installation or data migration. Inspect retained artifacts before dispatch. The
current workflow has no artifact-promotion path: a later authorised tag will
rebuild and rerun the release gates. Do not dispatch a duplicate candidate build.
