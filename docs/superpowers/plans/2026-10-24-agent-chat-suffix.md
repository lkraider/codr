# Fresh-agent chat suffix

**Goal:** Distinguish chats created from an existing project's agent/model chooser without renaming the original chat.

**Design:** New fresh-session chats use `<tab> (<workspace> - <agent>)`, including `codex`, `claude`, and `opencode`. Without a workspace label, use `<tab> (<agent>)`. Ordinary sessions and new-project workspaces keep their existing naming. Preserve the agent suffix when shortening long Unicode titles.

**State:** Save terminal-bound naming intent before starting the new agent. Once its native session is discovered, move that intent into its mapping in the same state save. Keep the mapping's suffix across restarts, pane moves, automatic metadata updates, and explicit rename-groups. Do not change manually renamed titles or global workspace labels.

## Execution

1. Add red tests for suffix formatting, byte limits, old-state defaults, terminal/backend matching, persistence, and launch naming intent.
2. Extend the existing formatter, add defaulted naming state, and use it in creation and both rename paths. Save launch intent with the existing start journal.
3. Test that the source mapping, queue, and title are unchanged. Update reference/changelog. Run Rust/Python suites, formatting, Clippy, and release build. Obtain two independent read-only reviews, fix demonstrated findings, and verify again.

No live group rename, publication, or worker restart. Keep idle-only activation.
