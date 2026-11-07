# Code-free project menu replies

The screenshot shows workspace selection requires a long token. A plain number currently falls through to agent decision routing. Bind short replies to the current persisted menu in the authenticated sender's chat, before permission/decision routing.

- Workspace, agent and model menus accept bare numbers. Workspace also accepts `new`; paginated menus accept `next` and `previous`.
- The same short choices work with `/new workspace`, `/agent`, and `/model`. `/new folder NAME`, `/model confirm`, and `/new confirm` require no code. Retain explicit coded commands for stale-menu protection and compatibility.
- Show readable instructions without long tokens. Creation/model changes still need a separate explicit confirmation; a bare number cannot confirm a launch or change a model.
- Resolve replies only against this chat's current draft. Keep expiry, sender, complete-text, attachment, voice, deduplication, queue, and final destination/model validation guards. Expired/out-of-range menu answers produce a menu error, never an agent answer. Ordinary text and numeric messages without a selection menu retain their existing behavior.
- Test number-to-decision collision, restart/deduplication, pagination, invalid and expired answers, unsafe input, and no-code confirmations. Run full verification and two independent read-only reviews. Do not publish the private screenshot.
- Activation must preserve state and avoid terminating the separately preserved OpenCode process or connector. Do not clear queued work.
