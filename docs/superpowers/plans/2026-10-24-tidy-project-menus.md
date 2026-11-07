# Tidy project menus

- Treat workspace/folder as the two location setup steps: `1 of 2 — Choose a herdr workspace:` and `2 of 2 — Folder name:`. Agent/model choices follow separately, not as fictitious steps 3/2.
- List existing workspaces first. Append `N+1. Create new workspace` after the workspace entries. Keep global numbering across pages, old `new` replies and explicit coded commands compatible.
- Hide page labels for one-page workspace/model lists; keep page indicators and next/previous only when needed.
- Add `0. Cancel` to editable project menus and pending launch/model confirmations. Resolve only within the authenticated chat and preserve uncertainty/replacement guards. Zero must never become a prompt or agent decision while setup is pending.
- At folder step accept the chosen name as plain text. Retain `/new folder NAME`; do not intercept slash commands such as `/help`. Keep existing folder validation and explicit final creation confirmation.
- Reject unsafe, expired, or invalid input without agent submission. Keep durable menus and deduplication. Never reset histories, queues, accounts, or uncertain requests.
- Test exact prompt order, numbering, folder routing, pagination, cancel in every step, unsafe replies, and no side effects. Verify full suites, formatting, Clippy, release build and two independent read-only reviews before applying.
