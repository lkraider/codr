# Portr desktop endpoint (local development)

Portr is an additional interface to the running codr plugin. herdr remains the
source of session identity and execution state. The phone path is unchanged.

The desktop app and its SSH connection are separate from codr. Codr contains the
server-side private endpoint, but it is **disabled by default**. Phone-only users
do not need portr, SSH, or a VPN.

An ordinary worker starts without desktop access:

```sh
herdr-white-noise-bridge run
```

During fresh interactive setup, the optional desktop/SSH question defaults to
No. Non-interactive setup enables it only with `python3 scripts/setup.py --portr`.
This requires a worker build with portr support. The setting is
`portr_enabled = true` in codr's configuration; an absent setting means false.
Setup does not install the desktop app, an SSH server, a VPN, or SSH keys.

This uses the same worker already supervised for the herdr plugin, not another
server to start. Do not run a second copy beside an existing service. Re-running
setup without a desktop option preserves an explicit existing preference.
`--no-portr` records a disabled preference. Changing this setting on an existing
installation reports that a worker restart is required; that option alone does
not restart a running worker. Apply the change only when all active work is idle.
Normal connector upgrades and explicit audio setup retain their existing service
handling. Configuration, state, and connector identity stay in place.

For a manually managed worker, `run --portr` enables desktop access for that run;
`run --no-portr` disables it even when the configuration enables it. The earlier
`--porter` and `--no-porter` flags remain aliases. The SSH client's
`porter --stream` command and wire protocol are unchanged.

## Trust boundary

When enabled, the bridge binds `<state file with .porter extension>/control.sock` in a private
0700 directory, with socket mode 0600. It binds only while holding codr's existing
exclusive state lock. On restart it can replace a stale socket, but never a
regular file or symlink. This endpoint grants control to the **same host user**
that already controls herdr and codr. It is not a multi-user remote gateway.

An SSH command on the same account invokes `"$HOME/.local/bin/herdr-white-noise-bridge" porter --stream`.
The CLI selects its standard `XDG_CONFIG_HOME` or `~/.config` configuration, just
as the existing worker does. A custom installation may override `--config`.
That command remains running and multiplexes bounded JSON lines over stdin/stdout.
It uses cheap local socket requests to the existing worker, not a new worker or
SSH connection per request. Omitting `--stream` retains the original one-request
CLI. Neither command opens bridge state for mutation or takes another worker lock.

The stream first subscribes with `{"version":1,"method":"watch"}` on the private
socket and emits `{"version":1,"event":"ready"}`. The worker publishes `changed`
events only when its session/queue projection changes, and `heartbeat` events
every 15 seconds. Invalidations carry no transcript or prompt content. They are
hints to fetch a live snapshot, not authoritative data or replayable history.
The worker reuses its existing three-second Herdr reconciliation; no independent
desktop polling loop or transcript reader is added. At most 16 subscriptions and
8 in-flight requests per CLI process are allowed. Slow writes time out after five
seconds; EOF or subscription failure ends the stream. Frames retain the 1 MiB
limit. Request replies can arrive out of order.

Each stream request includes a unique `id` for response correlation, independent
of the durable `request_id` used for prompt acceptance/receipt checks. The CLI
restores this `id` after its local socket exchange. The desktop reconnects with
reads only: outstanding requests fail on disconnect, and uncertain mutations are
held for explicit receipt checks. No request is automatically replayed. The
desktop requires stream support rather than silently falling back to one-shot SSH.

Use SSH over an existing Nostr VPN mesh address. Neither codr nor Portr starts
or reconfigures the VPN. SSH authenticates the device and host without exporting
the plugin's Nostr identity. Network access alone does not authorize requests.

## Version 1 requests

All requests have `version: 1`, `method` and a client `request_id`. Replies contain
`version`, the same `request_id`, and `result` (or a sanitized error code). The
local socket also uses the existing codr wire `id` correlation. Frames have the
existing one-MiB bound. SSH diagnostics and native reader errors are not relayed
as transcript content.

- `snapshot`: fetch live herdr agents and annotate existing mappings with queued,
  pending and uncertain state. Return the plugin's public `bridge_account` and
  current forwarding mode. Never substitute a stale successful snapshot.
- `history`: require `bridge_account`, `session_key`, `terminal_id`. Read the
  existing bounded recent-message excerpt through codr's transcript readers.
  Revalidate terminal identity after the read. Up to four desktop history reads
  run outside the bridge's input loop; more are rejected without blocking phone
  processing. Each excerpt message is capped at 64,000 UTF-8 bytes.
- `enqueue`: require `bridge_account`, `session_key`, `terminal_id`, a UUID
  `request_id`, and nonempty `text` of at most 30,000 UTF-8 bytes. Only existing
  active mappings with supported session readers are eligible. Save the prompt
  and its deduplication ID in the existing FIFO before acknowledging `queued`.
  An accepted ID returns `already_accepted`; no second item is added. The existing
  limit of 16 waiting prompts applies to phone and desktop input together.
  Reject the reserved phone controls `/queue`, `/clear-queue`, `/help`, `/sync`,
  `/status`, `unmute` and `/unmute` before insertion. They cannot enter the text
  FIFO and be mistaken for queue-control actions during dispatch.
- `receipt`: require the same account/session/terminal and UUID. Return whether
  the ID was durably accepted, without enqueueing or retrying anything. A missing
  receipt is not permission for automatic replay. A vanished herdr session or
  unavailable host requires manual inspection.

Queue mutations run in the same bridge loop as phone events. Desktop IDs use a
`porter:` prefix in the existing consumed-ID ledger. Submission uses the existing
queue dispatcher, which records intent and applies normal attribution, readiness,
completion and uncertain-send checks. The endpoint never directly calls
`agent.prompt`. It never changes forwarding mode.

## Limits

This endpoint does not implement per-repository write ownership. Neither a
working-directory label nor a per-session FIFO makes concurrent agent edits safe.
Coordinate repository writes on the host. Existing direct herdr input remains
possible and is not intercepted. The desktop currently exposes text queues and
recent excerpts, not all phone commands, media, decisions or lifecycle actions.

Portr's desktop code and development setup are in the adjacent `portr` checkout.
Tests in `tests/porter.rs` cover socket framing, permissions, request validation,
idempotency, slow readers and CLI forwarding. The bridge suite verifies a mixed
desktop/phone FIFO across restart using real sockets and synthetic agent output.
