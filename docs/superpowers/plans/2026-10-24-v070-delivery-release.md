# 0.7.0 delivery and release

## User contract

- Ordinary complete owner text from the phone is handed to the mapped agent immediately, including while it is working. Do not interrupt current work; the backend owns its native input scheduling. Keep identity, input-integrity, durable pre-submission consumption, and no-uncertain-retry checks.
- Existing accepted queue items, media preparation, and desktop endpoint receipts remain intact. They are never silently deleted to implement the new text path. Necessary download/transcription and disconnected transport recovery are not promises of immediate execution.
- `/cancel` is a complete standalone owner command that sends the native herdr `agent.interrupt` request to this chat's freshly verified native session. It never deletes chats, history, queued deliveries, or kills a worker. Persist intent before submission, acknowledge only a valid response, and never repeat an uncertain interrupt automatically. An idle session gets a no-running-task notice.
- `/clear-queue` is retired from help and supported actions. An old standalone invocation gets a retirement notice and never becomes agent text or deletes accepted work. `/queue` reports pending delivery state.
- Multiple phone follow-ups must preserve current response tracking and attribution through staggered transcript echoes and restarts. Proven pre-submission failure restores previous running state, not a false idle state.

## Release

- Preserve all earlier local features and stable-only fixes. Prepare 0.7.0 on a release branch; do not overwrite master/dev/linux or move published tags.
- Reconcile current GitHub master fixes, restore full Linux/macOS platform declaration, bump package/plugin/lock versions together, and update README, changelog, migration notes and synthetic screenshot provenance.
- Use real isolated demo interfaces with fictional data. Never capture or publish private chats or physical-device accounts whose demo isolation is unverified.
- Phone documentation must show the stock White Noise Android app on the connected ADB device, linked to real herdr through codr. Neither the separately implemented Odin/SDL Linux client nor rendered test fixtures satisfy this requirement. The initial desktop captures were rejected and must not ship. Keep publication on hold until replacement images are verified; a successful build alone does not clear this gate.
- Run full relevant local verification and two independent read-only reviews. Publish reviewed source to GitHub first, run the full four-target build/connector checks, and publish only successful 0.7.0 assets. Mirror exact source refs and unchanged GitHub assets/checksums to ngit/Blossom using devbox-amber with Tom's approval. Report blocked signing, replicas, CI import, or platform failures explicitly.
