# Pat onboarding

**Goal:** Newly created dedicated bridge identities publish the friendly display name Pat, and new session chats introduce Pat with `/help` guidance.

**Architecture:** Use the pinned MDK 0.14 `account_publish_profile` control request during fresh setup, with name and display_name set to Pat. Keep the internal `herdr-bridge` label, selected public key, account data, mappings, and relay settings unchanged. Replace only the existing new-group introduction text in the Rust worker.

## Contract

- Pat is a public Nostr profile name, not a replacement for the full npub used to verify invitations. Do not upload an avatar or include private project information in profile metadata.
- Only a bootstrap-confirmed newly created identity gets an automatic default profile. Reused accounts and existing installations keep their profiles; never reset a custom name.
- Record publication intent in the existing private `codr-bootstrap.json` before submission. Confirm the protocol, response type, account, name, and display name. Save a confirmed result only after a matching acknowledgement. Unconfirmed publication is reported and never blindly repeated by a retry or worker restart. Setup can still complete its phone configuration without pretending the name was published.
- Preserve the profile publication record when an interrupted setup reuses its original identity. A normal setup rerun and `--check` never publish the profile again.
- New groups say: `Hi, I'm Pat. I'll be delivering messages between White Noise and herdr for you. Type /help from any message to see what commands are available.` Keep the mapped session's existing connection context below the greeting.
- Use the current durable outbox path, with no extra per-poll greeting and no messages while muted. Existing groups/history are not rewritten or greeted again.

## Execution

1. Add red setup tests for exact profile payload and acknowledgement checks, preserved reused identities, pending publication, retry safety, and profile reporting.
2. Add red bridge assertions for the Pat welcome and preserved session context; implement the small setup helper and replace the existing introduction.
3. Update installation/reference/changelog text, including public profile metadata and continued npub verification. Test with fake Unix peers and setup runners only.
4. Run full Rust/Python tests, formatting, Clippy, and release build. Obtain two independent read-only reviews, fix demonstrated findings, and verify again.

No live profile publication, installation run, account creation, worker restart, commit, or release in this task. Preserve idle-only activation.
