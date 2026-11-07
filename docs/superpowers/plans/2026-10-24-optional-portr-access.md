# Optional portr desktop access

**Goal:** Keep the normal codr phone installation independent of SSH and make the server-side portr endpoint an explicit setup choice.

**Architecture:** Keep the existing private socket and stream implementation intact. Add a default-false `portr_enabled` configuration setting. The ordinary worker reads that setting, while explicit `run --portr` and `run --no-portr` flags override it. Retain the earlier `--porter` and `--no-porter` spellings as aliases. No SSH server, VPN, or separate desktop application is installed by codr.

## Setup contract

- Fresh non-interactive setup leaves portr disabled unless `--portr` is supplied. Phone-only setup omits the new setting so older release parsers still accept its configuration. `--no-portr` explicitly selects phone-only setup.
- Fresh interactive setup asks whether to enable optional portr desktop/SSH access, default No. Existing installations are not prompted again.
- Re-running setup without either option preserves an explicit existing setting; missing settings remain disabled. Explicit enable/disable changes only this setting, preserving comments and all other configuration. Validate the candidate before writing it privately.
- On an existing installation, record the requested setting and report that a worker restart is required. Do not restart a running worker merely to apply the desktop option. `--check` with a change option is rejected before mutation.
- Endpoint client commands and the persistent SSH stream remain compatible. The desktop app and SSH connection implementation remain in the adjacent portr project.

## Execution

1. Add red Rust tests proving an ordinary worker does not expose the socket, an opted-in worker does, and explicit flags override the configuration. Use a fake herdr server readiness signal rather than timing sleeps.
2. Add Python tests for fresh defaults, explicit opt-in, preserved upgrades, explicit disable, candidate validation failures, check-only safety, and CLI/interactive defaults.
3. Implement the configuration, runtime flag selection, and optional setup step without changing the endpoint protocol or phone path. Update installation, endpoint, reference, README, and changelog text.
4. Run the full Rust/Python suites, formatting, Clippy, and release build. Obtain two independent read-only reviews; fix demonstrated findings and verify again.

Do not run setup against the live installation, change SSH keys/services, disable Tom's running connections, clear queues, restart workers, commit, or publish. Activation remains idle-only.
