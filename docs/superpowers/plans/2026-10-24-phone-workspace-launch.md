# Phone workspace launch implementation plan

**Goal:** Let the configured phone owner create a workspace and start an agent after confirming the folder and launch details.

**Architecture:** Add an isolated `launch` command handler before ordinary prompt routing. Store one request per source chat. Use directory-relative, no-symlink filesystem operations and structured **herdr** API calls. Keep existing chat discovery, account data, and prompt queues unchanged.

**Stack:** Rust, serde, existing rustix filesystem support, existing Unix-socket clients, and Tokio tests.

## Command and safety contract

- `/new NAME FOLDER AGENT` previews a request. Names and path components use simple project identifiers. Agent choices are `opencode`, `codex`, and `claude`, without custom arguments.
- `launch_root` defaults to `~/code`. Relative folders resolve below that root. Absolute and `~/` paths must stay below the same root. The root must already exist. Reject symlinks, traversal, files, and the root itself.
- Preview makes no filesystem or workspace changes. Say **Folder already exists** or **Folder will be created**. Both cases require `/new confirm CODE` in the same chat within ten minutes.
- `/new cancel CODE` cancels an unconfirmed request. `/new status` reports the last request.
- Compare directory identities and missing components again before creation. If the folder changed, show a new preview and require a new code.
- Save confirmation intent before folder creation, workspace creation, and agent startup. Never repeat an uncertain operation after a timeout, duplicate event, or restart. Keep partially created folders and workspaces for inspection.
- Use `workspace.create` with an explicit cwd and `focus: false`, then `agent.start` on its returned root pane. Use a unique managed-agent name and no shell commands or agent arguments.
- The normal native-session discovery path creates the White Noise chat. Do not send an initial model prompt or create an alternate routing identity.
- Only complete standalone typed owner commands can launch. Captions, voice transcripts, other group members, and queued prompts cannot confirm a launch.

## Tasks

1. Add tests for the `launch_root` config default and override. Run red, then add the optional setting without changing old configurations.
2. Add request parsing, folder inspection/creation, and serialised launch phases in `src/launch.rs`. Add defaulted request state to `Mapping`. Test existing/missing folders, traversal, symlinks, and changed directory identities.
3. Add the owner command interceptor in `src/bridge.rs`. Test preview with no side effects, explicit confirmation, cancellation, expiry, duplicate delivery, and restart behaviour with fake Unix-socket peers.
4. Test lost workspace/start acknowledgements and preserve a paused, inspectable request instead of retrying. Verify that existing prompts and mute state do not change.
5. Document commands, limits, partial failure recovery, and native-session startup limits. Keep `/help` concise and reserve `/new` from desktop text queues.
6. Run the Rust suite, Python suite, formatting, Clippy, and diff checks. Request two independent read-only reviews. Fix demonstrated findings and verify again.

Activation remains subject to the user's wait-until-idle choice. Do not launch a real agent or create a real project as a test.
