# Guided workspace choice

**Goal:** Bare `/new` first offers an existing herdr workspace or a new workspace, then continues through folder, installed agent, model, and confirmation.

**Architecture:** Add bounded read-only workspace list/get helpers to `herdr.rs`. Extend the existing durable draft and launch request with defaulted workspace selections. Reuse the existing journalled `tab.create` and `workspace.create` paths rather than introduce another launcher.

## Contract

- `/new` shows a chat-bound paginated workspace snapshot, labelled with stable workspace IDs. `/new workspace CODE new` selects a new workspace; `/new workspace CODE N` selects an existing workspace. `next` and `previous` page ten workspaces at a time. Ordinary messages and bare numbers remain agent input.
- A failed workspace list is explicitly reported, not treated as proof that no workspaces exist. The new-workspace option remains available; `/new` can refresh the list. Menus still expire after ten minutes and survive restarts.
- Both choices continue to the existing confined folder picker, agent picker, model picker, and final confirmation. Existing workspaces receive a new tab with an explicit project cwd and label; no existing tab, process, workspace label, prompt queue, chat, or history is changed. New workspaces keep folder-derived names.
- Final confirmation shows the chosen workspace label and stable ID, or says that a new workspace will be created. Read the selected existing workspace again before folder/session/tab side effects. If it is gone or unavailable, stop without falling back to another or a new workspace. If its label changed, issue an updated preview with a new confirmation code.
- Preserve old saved drafts, old one-line `/new` requests, and the existing `/agent` same-workspace path through defaulted fields. An uncertain tab creation or agent start is not repeated after a restart. Existing-workspace chats retain the selected-agent naming suffix.

## Execution

1. Add red tests for the initial workspace menu and existing-workspace launch. Adapt prior new-project test helpers to explicitly select `new` and distinguish read-only workspace calls from mutations.
2. Implement list/get parsing, durable workspace menus, preview context, and preflight revalidation. Route existing destinations to `tab.create` and validate the returned workspace ID.
3. Add regression coverage for pagination, duplicate/stale/foreign/truncated replies, restart recovery, absent or renamed workspaces, list failure, saved-field defaults, preserved source state, and uncertain tab acknowledgements. Use fake Unix peers and API executables only; no real session launch.
4. Update reference/changelog. Run full Rust/Python suites, formatting, Clippy, and release build through fastbuild. Obtain two independent read-only reviews, fix demonstrated findings, and verify again.

No live creation, group rename, service restart, commit, or publication. Activation remains idle-only.
