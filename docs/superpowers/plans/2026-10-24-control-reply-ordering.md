# Control reply ordering and focused captures

**Goal:** Keep control replies after their phone requests, then show one clear action per stock Android capture.

**Evidence:** The demo phone is approximately 0.86 seconds ahead of the host. MDK records whole-second message timestamps. Actual `/help` and its reply share a timestamp. The cancellation reply is one second earlier than its request.

**Scope:** Do not change device clocks, agent prompt delivery, interruption, account identities, existing history, or normal installed services. Use the assigned checkout. Keep the release on hold until new images are verified.

## Implementation

- [x] Add failing regressions in `tests/bridge.rs`: a valid owner input records a durable per-chat `manual_reply_after` deadline, foreign and implausibly future inputs do not extend it, delayed manual replies survive restart without blocking other chats, and native prompts remain immediate.
- [x] In `src/state.rs`, add `Mapping::manual_reply_after: u64` with `#[serde(default)]`. Existing saved mappings default to zero.
- [x] In `src/bridge.rs::route_inbound`, after owner, mapping, message ID and deduplication checks, advance the deadline to `recorded_at.saturating_add(1)` only when the timestamp is no more than five seconds ahead of `now()`. Never consume or defer agent input to enforce this deadline.
- [x] In `Bridge::flush`, skip only manual outgoing items whose own group's deadline is in the future. Continue processing other items and chats. Keep all durable replies and existing idempotency keys intact.
- [x] Run targeted tests, the complete Rust/Python suites, formatting, strict clippy, and two independent read-only reviews. Document the five-second skew bound and whole-second ordering. Verified 312 Rust tests, eight ignored, and 92 Python tests.

## Capture and delivery

- [x] Rebuild the reviewed worker. Replace only the isolated demo worker, without stopping herdr agents, the connector, or the normal installed bridge. Preserve its state.
- [x] Create separate real, empty native sessions for two fictional demo chats. Capture `/new` with workspace choices in one chat. Capture `/model` with provider choices in the other. Do not include help or cancellation in either detail crop. Link full app views for context.
- [x] Verify request/reply timestamps and actual rendered order before capture. Use the connected Pixel 5 and unchanged stock White Noise app. Do not rearrange pixels or clear prior history.
- [x] Add short purpose-based captions. Remove the earlier mixed-history images from the new gallery. Keep help as text reference documentation.
- [x] Verify native pixels, metadata, privacy, file paths and hashes. Obtain two read-only reviews.
- [ ] Publish reviewed source to GitHub, mirror approved refs, and continue full release verification before publication.
