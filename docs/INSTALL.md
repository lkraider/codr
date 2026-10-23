# Install **codr**

**codr** is a **herdr** plugin backed by two per-user services: the Rust bridge and its MDK transport. Linux uses systemd; macOS uses launchd LaunchAgents.

## One-prompt installation

Give your coding agent this prompt, with your own complete public key:

> Install https://github.com/tidley/codr on this machine. My White Noise public key is npub1…. Follow the agent installation guide and report what you verified.

The [agent section](#instructions-for-coding-agents) provides the non-interactive setup steps.

## Requirements

- Linux x86_64 or aarch64, or macOS on Apple silicon or Intel.
- glibc 2.35 or newer for the dedicated Linux connector.
- Python 3.11 or newer and Git. On macOS, setup re-executes through a newer Homebrew interpreter when `/usr/bin/python3` is 3.9.
- FFmpeg for voice prompts. Use `--no-audio` for a text/image-only installation.
- A working systemd user manager under the account that runs **herdr**, or a logged-in launchd session on macOS.
- **herdr** 0.9.1 or newer, with a running local server.
- A working, authenticated OpenCode, Codex, or Claude Code installation.
- **herdr** native session IDs and access to the selected agent's local transcripts.
- White Noise on your phone and your own public `npub`.
- Access to GitHub release assets and the configured White Noise relays.

Agent authentication stays in its existing location. Setup does not request a provider API key, `nsec`, seed phrase, or private key.

The phone account must publish a compatible Marmot KeyPackage before the bridge can invite it. Open White Noise and let it connect to its relays.

## Install through **herdr**

Run these commands as your normal user, not root:

```bash
herdr plugin install tidley/codr --ref v0.6.0
herdr plugin action invoke local.white-noise.setup
```

**herdr** previews and runs the manifest's installation hook, then registers the plugin. The hook downloads the bridge and dedicated connector for the host architecture and checks their SHA-256 checksums. Users do not need Rust or Cargo.

The Setup action opens a **herdr** tab. Enter your own White Noise public `npub` there. Setup:

1. Validates the key through **codr**'s configuration parser before changing accounts or services.
2. Checks the **herdr** socket, available supported agents, Git, and the host service manager. Codex-only and Claude-only installations do not need OpenCode.
3. Selects the release-matched connector in **codr**'s own binary directory.
4. Creates or reuses the dedicated `herdr-bridge` identity under `~/.marmot-agents/herdr`.
5. Writes owner-only configuration and dedicated systemd units or launchd agents.
6. Starts the transport and bridge, then prints the bridge's public `npub` and local result.

The dedicated connector uses a pinned MDK revision with **codr**'s founding-admin and media-recovery patch. See [connector source and provenance](../patches/README.md). Setup does not replace a shared `wn-agent` executable.

The binary installer selects the **codr** version from the installed plugin checkout. A pinned installation uses that exact version's assets and checksums. A missing release asset or checksum stops installation rather than falling back to unverified code.

### Select a **herdr** server

The setup tab receives `HERDR_SOCKET_PATH` from its **herdr** server. For direct setup outside a tab, pass `--herdr-socket /absolute/path/to/herdr.sock` or set `HERDR_SOCKET_PATH` explicitly.

One **codr** configuration targets one **herdr** server. Select the intended server before installation when using named sessions:

```bash
export HERDR_SOCKET_PATH="/absolute/path/to/the/intended/herdr.sock"
herdr api snapshot
```

### Existing installations and upgrades

Setup preserves existing configuration, identities, mappings, and customised service definitions. A supplied owner that differs from the configured phone account is rejected.

Version 0.6.0 enables automatic handoff by default: a new local native prompt mutes all groups, and a new authorized phone message resumes forwarding. Existing history is only a baseline on first observation. Set `auto_forwarding = false` in the bridge configuration before restarting its worker if you prefer manual local-activity control. See [controls and polling limits](REFERENCE.md#at-desk--away-forwarding).

For an existing installation, setup validates the configuration and selects the release-matched connector. A managed transport upgrade pauses the bridge and transport, replaces only the transport executable in its unit, then restarts both services. Configuration, identities, and mappings are preserved. Custom transport overrides require inspection before an upgrade. `--check` validates without changing services.

Setup stores the OpenCode executable's PATH entry, such as `/opt/homebrew/bin/opencode`, instead of Homebrew's versioned Cellar path. If an upgrade removes an absolute configured path and a replacement is installed, rerun setup: it validates session readers with the replacement before saving it, then restarts the worker. A failed restart restores the original configuration so setup can retry. `--check` reports an available repair without writing or restarting.

Bare command names such as `opencode` keep their normal PATH lookup. Claude-only and Codex-only installations do not require OpenCode, including on repeat setup and `--check`. If OpenCode is unavailable, validation still fails when a live session needs its reader; without such a session, validation can be deferred.

The binary installer replaces **codr**'s executable atomically. A running worker continues with its previous executable until restarted. Run setup first to select the matching connector. After that, a separately replaced bridge executable can be applied by restarting only the **codr** worker when work and group creation have settled:

```bash
# Linux
systemctl --user restart herdr-white-noise-bridge.service
# macOS; this reloads the agent and its configuration
launchctl kickstart -k "gui/$(id -u)/io.github.tidley.codr.herdr-white-noise-bridge"
```

**herdr** refuses a GitHub install over a locally linked copy of the same plugin. To switch from a local checkout to managed releases, inspect the current installation, then unregister the local plugin before installing the release:

```bash
herdr plugin list
herdr plugin unlink local.white-noise
herdr plugin install tidley/codr --ref v0.6.0
```

Unlinking leaves the local checkout and account state in place. Do not delete the MDK home or reset the bridge to perform an upgrade.

### Interrupted first setup

Re-run setup with the same owner and socket. Matching service files are reused. If the MDK account already exists, setup selects its account ID and bootstraps with `--no-create`.

Ambiguous accounts or service files that differ from the proposed fresh configuration require inspection. Setup reports the conflict instead of overwriting them. Do not remove persistent state to make installation pass.

## Verify on the phone

1. Compare the full bridge `npub` printed by setup with the inviter in White Noise.
2. Accept a session-group invitation from that identity.
3. Send `/status` and check that it reports a connected session.
4. Send a harmless request with a fresh word. Ask the agent to echo it without tools or file changes.
5. Check that the intended **herdr** session receives the prompt and its final reply returns to the same group.

Group creation can take time. Group names and initial connection/status messages provide the normal visible installation signal. They do not prove the phone-to-agent round trip.

An extra completion notice is optional. If requested, follow the [completion-notice instructions](REFERENCE.md#completion-notice), select a verified group ID, and distinguish an MDK-confirmed send from phone-confirmed receipt.

## Diagnostics

```bash
herdr plugin list
herdr-white-noise-bridge status
# Linux
systemctl --user status wn-agent-herdr.service herdr-white-noise-bridge.service
journalctl --user -u wn-agent-herdr.service -u herdr-white-noise-bridge.service --no-pager -n 50
# macOS
launchctl print "gui/$(id -u)/io.github.tidley.codr.herdr-white-noise-bridge"
launchctl print "gui/$(id -u)/io.github.tidley.codr.wn-agent-herdr"
tail -n 50 ~/Library/Logs/codr/wn-agent-herdr.err.log ~/Library/Logs/codr/herdr-white-noise-bridge.err.log
```

Configuration lives under `${XDG_CONFIG_HOME:-~/.config}/herdr-white-noise-bridge/config.toml`. The worker's service definition specifies that path explicitly. State uses `${XDG_STATE_HOME:-~/.local/state}/herdr-white-noise-bridge/state.json`.

Set `opencode_db` to select incremental, read-only OpenCode database reads. Select the database for the mapped sessions explicitly. An incompatible history format selects CLI export automatically, including after an OpenCode upgrade. Database access, corruption, and locking errors still stop collection. See [reader configuration](REFERENCE.md#incremental-opencode-database-reads).

For Codex profiles, `codex_home` selects the directory containing `sessions` and `archived_sessions`. It defaults to `CODEX_HOME` or `~/.codex`. For Claude Code, `claude_home` selects the directory containing `projects`; it defaults to `CLAUDE_CONFIG_DIR` or `~/.claude`. Use absolute paths. Setup saves the installing shell's roots for new configurations, and preserves existing configurations during upgrades.

If a supported agent appears without a native session ID, check its **herdr** integration. Enable the matching **herdr** integration when needed rather than guessing a transcript from its terminal title or repository. **codr** does not copy credentials or reconfigure the model provider.

Native transcript records are bounded to 16 MiB. Codex transcripts are bounded to 128 MiB per file. Claude Code transcripts have no file limit. The bridge parses a Claude Code transcript once and then parses only the bytes appended since the last read. It parses the whole file again when the file is replaced or its last parsed bytes change. Missing, ambiguous, or unsupported transcript data stays unavailable for final-reply extraction. Claude Code must record an explicit `end_turn` stop reason; the bridge retries later if the writer has not persisted it yet.

See [Recovery and diagnostics](REFERENCE.md#recovery-and-diagnostics) for uncertain group creation or prompt submissions. Do not blindly repeat those operations.

## Instructions for coding agents

### Required input and discovery

Get the user's own complete White Noise **public** `npub`. Never use a test fixture or another person's public key as the owner. Do not request private keys or provider credentials.

Before changing the host:

1. Verify the OS and architecture, Python 3.11+, Git, the systemd user bus or launchd session, and executable paths.
2. Inspect existing **codr** configuration, services, identities, and plugin registration.
3. Resolve the intended **herdr** socket and verify it with `session.snapshot`. Ask which server to use if several are plausible.
4. Check that at least one supported agent already works under this user and reports a native **herdr** session reference. Do not change model authentication or execution permissions.

### Non-interactive installation

Set `OWNER_NPUB` to the public key supplied by the user. Export the selected `HERDR_SOCKET_PATH` before running **herdr** commands.

```bash
herdr plugin install tidley/codr --ref v0.6.0 --yes
PLUGIN_ROOT="$(herdr plugin list --plugin local.white-noise --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["result"]["plugins"][0]["plugin_root"])')"
python3 "$PLUGIN_ROOT/scripts/setup.py" --npub "$OWNER_NPUB" --herdr-socket "$HERDR_SOCKET_PATH"
```

Use [Existing installations and upgrades](#existing-installations-and-upgrades) instead of overwriting a local plugin registration. For a local source checkout, the same setup script works after the binary has been installed.

For a passive check of an existing configuration:

```bash
python3 "$PLUGIN_ROOT/scripts/setup.py" --check
```

Do not overwrite customised configuration or service definitions, broaden sender permissions, share another harness's account home, or restart unrelated agents to make checks pass.

### Completion report

Report the installed revision/version, configuration path, selected **herdr** socket, service status, bridge public `npub`, and mapped-session count. Use the bridge's `status` command to inspect mappings after setup.

State exactly which checks ran. Report the phone round trip as pending unless the user or observed inbound/reply traffic confirms it. Never include private keys, provider credentials, prompts, or exported transcripts.

If a prerequisite or release asset is unavailable, report that blocker precisely rather than claiming installation succeeded.

## Build from source

This path requires Rust and Cargo. Use the target host's build/storage policy. Audio setup needs FFmpeg and downloads the CPU or MLX runtime plus the approximately 164 MB model. On Linux its service is capped at 1 GiB RAM; launchd on macOS has no equivalent cap.

```bash
git clone https://github.com/tidley/codr.git
cd codr
git checkout master
cargo test --locked --tests
cargo install --locked --path . --root "$HOME/.local"
herdr plugin link "$PWD"
python3 scripts/setup.py --npub "$OWNER_NPUB"
```

`plugin link` does not run the download hook. Keep the linked checkout at its permanent location. The startup hook starts the configured worker after **herdr** restores its session; a client disconnect does not stop it.

Use `python3 scripts/setup.py --npub "$OWNER_NPUB" --no-audio` to skip speech setup. Existing installations can add it with `python3 scripts/setup.py --phonon`. See [media and speech details](REFERENCE.md#images-and-voice-messages).
