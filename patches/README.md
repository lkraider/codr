# Connector source

**codr** ships a dedicated `wn-agent` built from the MDK revision in [`connector.toml`](../connector.toml).
The release workflow applies [`mdk-codr.patch`](mdk-codr.patch) before building it.

The current pin is MDK 0.14.0 at `3fb4b4759ac676b0c21c4c3527074866c9fa4c89`.
Both full releases and Linux previews build the connector from this source.
The existing patch applies unchanged; the two extensions are not included in upstream 0.14.0.

Before the first 0.14 start, stop the bridge and connector and keep a private, device-local
copy of the complete connector home, including database sidecars, media, configuration,
and secret-store credentials. MDK rekeys databases and advances their schema on first open;
0.13 cannot open the upgraded data. Do not roll back only the executable or automatically
restore an old account snapshot after new MLS traffic. See the upstream
[migration guide](https://github.com/marmot-protocol/mdk/blob/v0.14.0/docs/integration/0.14.0.md#storage-upgrade).
Keep the account directory out of cloud and device-sync backups.

The patch adds founding admin selection through `group_create_with_admins` and retains the local `include_media`
history-recovery extension. Existing `group_create` clients keep creator-only defaults. Non-member admins are refused
before creation. The engine assigns the owner and creator roles in the founding state, including the initial Welcome.

The bridge probes `group_create_capabilities` before recording creation intent, so an older connector can be upgraded
without leaving a false in-flight creation marker.

The patch includes protocol and relay-backed tests. The connector archive contains the upstream license and source
provenance. The installer verifies its checksum and uses a versioned, **codr**-owned path. It does not replace a shared
`wn-agent` executable.
