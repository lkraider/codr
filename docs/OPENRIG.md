# **codr** with OpenRig

This guide is the map for running **codr** against OpenRig seats: what to install, how a seat becomes a phone chat, which settings matter, and what to check when something does not arrive. It links to the sections that already hold the details instead of repeating them.

Facts here are checked against the code of this fork. Anything that could not be checked is marked **not verified**. Every macOS step is **not verified on macOS**: the Linux host that wrote this guide cannot run launchd.

## What you get

One White Noise phone group per listed OpenRig seat. A message you send in the group goes to the seat as input. When the seat finishes the turn that answers it, the final reply comes back to the group. Replies to other agents or to the desk stay off the phone (see [Settings that matter](#settings-that-matter)).

The OpenRig bridge is a separate installation. It has its own identity, configuration, state and services, and it never touches a **herdr** bridge on the same host.

## What it needs

| Need | Why |
| --- | --- |
| A running OpenRig daemon and `rig` on the bridge host | Seats come from `rig ps --nodes -A --json --full`; prompts go through `rig send <seat> --raw`; `rig seat status <seat> --json` gives each seat's Claude home. |
| `node` | The `rig` executable starts with `#!/usr/bin/env node`. |
| `curl` | The bridge follows the daemon event stream at `<openrig_url>/api/events` with `curl`. |
| `tmux` (optional) | Detects a seat pane in copy mode. Without it prompts are sent as usual; the reference says the cause is logged once (**not verified** in code). |
| FFmpeg (for voice) | Both transcription scripts convert recordings with it. |
| Claude Code seats | The bridge reads Claude Code transcripts. |

## Install

Install the **codr** binary first ([Build](REFERENCE.md#build) or [Install](INSTALL.md)). Then run setup with each seat that gets a phone group:

```sh
python3 scripts/setup.py --npub "$OWNER_NPUB" --seat seat-a@my-rig --seat seat-b@my-rig
```

Setup prints the bridge `npub`. Accept its invitation on the phone ([Verify on the phone](INSTALL.md#verify-on-the-phone)). `python3 scripts/setup.py --openrig --check` validates the configuration and the seat readers without changing services. Running setup again replaces the seat list with the seats you pass, so repeat every seat you want to keep; each new seat gets a new group.

| | Linux (systemd, user units) | macOS (launchd) |
| --- | --- | --- |
| Services | `wn-agent-openrig.service`, `openrig-bridge.service` | LaunchAgents `io.github.tidley.codr.wn-agent-openrig` and `…openrig-bridge` |
| Configuration | `${XDG_CONFIG_HOME:-~/.config}/codr-openrig/config.toml` | same path |
| State and speech folder | `${XDG_STATE_HOME:-~/.local/state}/codr-openrig/` | same path |
| Logs | `journalctl --user -u openrig-bridge.service` | `~/Library/Logs/codr/` |
| Status | verified on Linux | **not verified on macOS** |

The full list of files setup writes, how to move a seat, and how to remove the bridge are in [Install the OpenRig bridge](INSTALL.md#install-the-openrig-bridge). After you move or upgrade `node`, update `rig_bin` in the configuration and the PATH in the bridge service, then restart the bridge.

## Seats and chats

- Each listed seat that runs Claude Code gets one group. The group title is the seat name, for example `seat-a@my-rig`, with no `[cd]` prefix.
- The group stays with the seat when the seat starts a new Claude session. The bridge reads the new transcript; replies already in it stay off the phone, except a reply to a phone prompt.
- Only the configured owner can prompt a seat. Other members are ignored.
- `/status` and `/queue` work in a seat chat ([chat commands](REFERENCE.md#white-noise-chat-commands)). `/cancel` clears pending menus, but its interrupt goes through the **herdr** socket, which this bridge does not use, so by the code it cannot stop an OpenRig seat (code read, not run). Stop a seat at the desk.

Details: [OpenRig seats](REFERENCE.md#openrig-seats).

## Settings that matter

Edit `config.toml`, then restart `openrig-bridge`. Setup writes `backend`, `openrig_seats`, `rig_bin`, `phone_replies_only` and `claude_home`, plus `speak_dir`. It does not set `transcribe_command`, `openrig_url` or `tmux_bin`; the other settings keep their defaults.

| Setting | What it does |
| --- | --- |
| `backend = "openrig"` | Selects OpenRig seats instead of **herdr** panes. `herdr_socket` is not used. |
| `openrig_seats = ["seat-a@my-rig"]` | The seat session names that get a phone group. At least one is required. |
| `phone_replies_only = true` | Sends a final reply only when its turn holds a prompt that came from the phone. |
| `rig_bin` | The `rig` executable. Setup stores the resolved path. |
| `openrig_url` | Daemon base URL; default `http://127.0.0.1:7433`. |
| `tmux_bin` | The `tmux` executable; default `tmux`. |
| `claude_home` | Fallback Claude config directory for a seat with no reported home. |
| `settled_reread_secs` | How long the bridge rereads a transcript after a `Stop` hook while the reply is not yet written; default 30. |
| `transcribe_command` | The voice transcription command (see [Voice notes](#voice-notes)). |

**Why `phone_replies_only` is required for OpenRig seats.** Other agents type into a seat with `rig send`. Without the setting, every final reply of the seat reaches the phone, including replies to those peers. With it, the bridge matches the transcript text of a prompt to the phone submission and sends only the reply of the turn that holds it; working and idle notices follow the same rule, blocked notices always go through. If peer replies show up on the phone, check this setting first. The rules for held prompts, the 30 minute limit and Claude Code's queued prompts are in [Phone replies only](REFERENCE.md#phone-replies-only).

**Per-seat Claude home.** Each seat's transcript is read from the `runtime_home` (`CLAUDE_CONFIG_DIR`) that `rig seat status <seat> --json` reports, so seats on different Claude accounts each work. A seat with no reported home, or a failed lookup, is read from `claude_home`. This is the configured launch home, not proof of the login a running session uses.

## Voice notes

Setup for the OpenRig bridge refuses audio options and writes no `transcribe_command`, so add the command by hand to `config.toml` and restart the bridge. **codr** stops a transcribe command after 600 seconds.

| Host | Command | Notes |
| --- | --- | --- |
| Linux CPU, audio must stay local, several languages | `scripts/transcribe_local.py` (faster-whisper `medium`, int8) | Needs Python `venv`, FFmpeg and about 1.5 GB of disk. About 1.3 times real time after a 15 to 20 second model load on the one six-core host measured; a five-minute note takes about seven minutes. Setup steps and the `small` fallback: [Local multilingual transcription](REFERENCE.md#local-multilingual-transcription). |
| A host that allows the cloud | `scripts/transcribe_gemini.py` | The audio leaves the machine and goes to Google. Detects the language. [Gemini transcription](REFERENCE.md#gemini-transcription). |
| English only | Phonon-2 | Local and small, but English only. Its installer (`setup.py --phonon`) belongs to the **herdr** install and is not available for the OpenRig bridge: **not verified** with it. [Local speech worker](REFERENCE.md#local-speech-worker). |

Speed numbers come from one host; measure yours. Whether `--hint` helps mixed-language speech has not been measured.

## Updates

Build **codr** from this fork and run setup again; it repairs services and keeps the identity, configuration and state. Do not use `/update` from the phone for this build: the updater requires the exact connector pin and patch it expects ([Confirmed updates from ngit](REFERENCE.md#confirmed-updates-from-ngit)), and this fork changes `patches/mdk-codr.patch`, so its patch hash differs from upstream's. **Not verified**: what `/update` does with the fork's connector in practice.

## When something does not arrive

| Symptom | Check |
| --- | --- |
| Nothing reaches the phone after a seat finishes | Event stream: the log shows one line when it fails, then retries up to 30 s; replies still come from the 60 s `rig ps` read. Check `curl -sN <openrig_url>/api/events` reaches the daemon. |
| Events stop after an OpenRig database reset | The stored event number is above every new one. Stop the bridge, delete `openrig_event_seq` from the state file, start it. |
| Peer replies appear on the phone | `phone_replies_only` is off or the seat is read from the wrong home. |
| A reply to a phone prompt is missing | The prompt may have waited more than 30 minutes (the phone gets a notice), or the desk typed the same text first. |
| "answer the prompt at the desk and resend" | The seat waits on an interactive prompt; OpenRig refuses input. Answer at the desk. Other refusals, for example `typing_guard_enabled`, are quoted to the phone; the prompt is not retried. |
| "This message is held" | The seat pane is in tmux copy mode. React 👍 to the notice to leave copy mode and send, or wait until copy mode ends. It never leaves copy mode without the reaction. |
| Phone shows the seat as blocked after an approval at the desk | The reference says an approval at the desk sends no hook, so the state corrects at the next hook, usually `Stop`. **Not verified** in code. |
| Voice note: transcription failed | Run the script by hand with an audio file: **codr** discards its stderr. The Phonon script refuses recordings over five minutes; the bridge itself enforces no length limit, only the 600 s command limit, and the reference's "five minutes" for the Gemini and local scripts is not checked in their code. On a slow host the old 180 s limit cut long notes; the limit is now 600 s. |
| "Media from this group is still being prepared" | One voice note or image is processed at a time; resend the text when the note is done. Other groups are not delayed. |
| Seat on another Claude account shows no replies | `rig seat status <seat> --json` has no `runtime_home`, so `claude_home` is used. |
