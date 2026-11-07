# Guided projects and model selection

**Goal:** Guide the phone owner from `/new` through folder, installed agent, model, and confirmation; provide `/agent` and `/model` in existing chats without losing history or queues.

**Architecture:** Keep confirmed filesystem and herdr operations in `launch.rs`. Add `catalogue.rs` for installed executables and backend model discovery, and `launch_flow.rs` for durable chat-bound menus. Reuse authenticated OpenCode CLI API calls; never change global model settings or type backend slash commands into a terminal.

## Contract

- Bare `/new` asks for a folder using `/new folder CODE NAME`. No folder is created yet. The folder basename supplies the workspace label.
- `/agent CODE NUMBER` chooses from detected installed OpenCode, Codex, and Claude Code executables. `/model CODE NUMBER` chooses from a stored catalogue snapshot. Codes bind each menu to the source chat and change between steps. No bare number or ordinary text is intercepted.
- Model pages contain ten choices; `/model CODE next` and `/model CODE previous` navigate. Include the backend default when launching. OpenCode choices come from enabled, non-deprecated `/api/model` rows for the target location. Codex choices come from its local model cache, labelled as cached. Claude offers documented aliases, labelled as aliases rather than verified account entitlements. Do not invent a live discovery API.
- `/new cancel` cancels a menu, and menus expire after ten minutes. Existing `/new NAME FOLDER AGENT` and code-bound launch confirmation remain supported. Confirmations show folder existence, agent, and selected model. Revalidate installed agent and selected catalogue model before side effects.
- Persist all menu steps and message IDs before acknowledgement. Duplicate or stale replies cannot advance a newer menu. Attachments, transcripts, truncated input, foreign senders, and desktop queue input cannot select or confirm.
- `/agent` from an existing chat uses the current mapped directory, starts a fresh agent in a new tab of the same workspace, and eventually creates a separate chat. Old session, history, and waiting prompts remain unchanged. Keep the existing launch-root confinement.
- `/model` in an OpenCode V2 chat lists its available models and asks `/model confirm CODE` before changing the same native session. Require an idle live identity and no pending or queued bridge input at confirmation. Persist intent before the API call, read back the selected model, and never repeat an uncertain switch.
- `/model` in Codex or Claude starts the model-selection part of a fresh-session flow, with a visible separate-chat warning and final confirmation. No in-place backend control is assumed.
- OpenCode launch with a selected model first creates a native session through `/api/session` with its location and Model.Ref, then launches `opencode --session ID` through herdr. OpenCode V2 does not expose a top-level model flag. Persist the new session ID and creation phase before submission; unknown outcomes require local inspection. Codex and Claude receive structured `--model ID` launch arguments only.

## Tasks and verification

1. Add red tests for catalogue filtering, installed-executable detection, command reservation, bare `/new`, persistence, page navigation, and safe confirmations. Implement the catalogue with bounded output reads and no model calls.
2. Add defaulted request/menu state. Implement the folder/agent/model menus and reuse the existing launch confirmation. Test restart, stale codes, old state, expiry, cancellations, owner scope, and unchanged queues/mute state.
3. Extend launch execution for fixed model arguments, native OpenCode session creation, and same-workspace new tabs. Test exact API payloads and lost acknowledgements with fake peers only.
4. Add in-place OpenCode model selection with durable intent, live identity/idle checks, and readback. Test busy/queued rejection, removed models, duplicate confirmations, persistence failure, and uncertain responses.
5. Update help/reference/changelog. Run `cargo test --locked -- --test-threads=1`, Python unittest discovery, formatting, Clippy, and release build via fastbuild adapters. Request two independent read-only reviews, fix demonstrated findings, and rerun verification.

Do not commit unrelated changes, start a real model session for tests, publish changes, or restart the shared worker. Activation still waits for idle.
