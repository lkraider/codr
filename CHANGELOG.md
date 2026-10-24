# Changelog

## Unreleased

### Changed

- Accept phone prompts while the agent works. A blocked agent still refuses them with the terminal-prompt notice. A prompt sent while media from the same group is being prepared gets its own notice.
- Add `settled_reread_secs` to set the transcript reread interval for settled agents. The default stays 30 seconds.

### Fixed

- Keep stable executable paths across Homebrew upgrades and report missing OpenCode executables without exposing transcript data.
- Preserve optional OpenCode support for Claude-only and Codex-only installations, including repeat setup and check mode. Bare commands retain PATH lookup.
- Validate replacement session readers before saving an OpenCode repair. Restore the old configuration after a failed worker restart so setup can retry the repair.

## [0.6.0] - 2026-10-22

### Added

- Automatically mute all groups on local native prompts and resume on authorized phone input. Preserve manual controls without opening a startup popup.
- macOS support for the Rust bridge, the MDK transport, and the local Phonon-2 worker.
- launchd LaunchAgents on macOS, with the same restart-on-failure behaviour as the systemd units.
- Native macOS aarch64 and x86_64 release binaries, ad-hoc signed, alongside the existing musl builds.
- Setup finds a compatible interpreter for Phonon, including Python 3.10–3.12 on Intel macOS.
- Phonon installs its MLX runtime on Apple silicon and the CPU runtime on Intel Macs.
- New groups grant admin rights to the configured phone owner and bridge in the founding state, including the initial Welcome.
- Release-matched connector binaries, built from a pinned MDK revision and the published founding-admin/media-recovery patch.
- Managed connector upgrades that preserve account state, group mappings, and transport arguments. Custom overrides require inspection.

### Changed

- Service supervision moved behind one interface shared by systemd and launchd.
- The install hook and setup tolerate macOS's Python 3.9 when only stdlib parsing is needed.
- Group names include the workspace, with UTF-8 byte limits.
- OpenCode session exports support both 1.x and 2.x formats.
- Live group titles follow **herdr** label changes, with durable rename intent and protection for manually renamed groups.
- Session readers select CLI export when an OpenCode database history format becomes incompatible, including upgrades while the worker runs. Storage errors still defer reads.
- Setup checks live transcript readers before changing services or identities. `check-readers` repeats that read-only validation without printing transcripts.

### Known limits

- Automatic handoff observes native transcripts after polling and read latency. It cannot recall output already submitted to MDK. Set `auto_forwarding = false` to retain manual local-activity control.
- Phone prompts are not queued behind busy or blocked agents. Wait for idle and resend; supported decision answers remain available while blocked.
- launchd has no memory cap, so the 1 GiB Phonon limit applies only on Linux.
- macOS setup needs a logged-in launchd session; a headless SSH session cannot bootstrap the agents.

### Compatibility

- Upgrade the bridge and dedicated connector together through setup. Shared `wn-agent` installations are not replaced.
- Existing groups retain their current admins. Group names do not affect routing, and additional admins do not receive agent-prompt permission.
- On Linux, the dedicated connector requires glibc 2.35 or newer; its OpenSSL dependency is bundled.

## [0.5.0] - 2026-10-22

### Added

- Image attachments with captions and private local files for the mapped agent.
- Local Phonon-2 voice transcription, submitted directly as an agent prompt.
- A quoted transcript beneath `transcribed`, edited into the original `transcribing` bubble when MDK supports edits.
- Optional incremental, read-only OpenCode SQLite reads through `opencode_db`. Unchanged sessions fetch no message or part bodies.
- Background transcript reads, with up to four readers and one outstanding read per session.

### Changed

- Chat names use `[cd] session`. Descriptions contain workspace and agent details.
- Status notices use blockquotes. Task lists appear below the status without a `Tasks` heading.
- README screenshots cover images, voice, decisions, and shared chats with owner-only agent control.

### Fixed

- Pending and prepared media survive restarts and temporary **herdr** outages without duplicate prompt submission.
- Deferred media in one session does not block preparation for other chats.
- Transcript reads preserve session identity and forwarding-mode checks. Exiting sessions get a fresh final read.
- Transcription notice delivery retries independently of agent submission, including lost send or edit responses.

### Compatibility

- Same-bubble transcripts and editable progress require `edit_responses = true` and an MDK build with `edit_message` support.
- Offline media recovery needs MDK's opt-in `include_media` download references. These extensions are not in published MDK 0.13.0 binaries.
- Incremental database reads were verified against OpenCode 1.19.3's v1 projections and durable event log. The CLI reader remains available when `opencode_db` is unset.

[0.6.0]: https://github.com/tidley/codr/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/tidley/codr/compare/v0.4.0...v0.5.0
