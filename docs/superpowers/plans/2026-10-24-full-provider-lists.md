# Provider-first model lists and readable help

- New model selection menus first offer Backend default where supported and provider categories, even when only one provider has models. A default-only catalogue still offers that sole choice. Do not invent a default-model switch for existing sessions that cannot apply it.
- Selecting a provider/category shows every available base model in that list, without next/previous or a page counter. Available variants stay in their separate selection step and also show the full list. Keep back and zero-cancel.
- Reuse the existing durable 30,000-byte, Unicode-safe outbox segmentation for lists above the encrypted transport limit. Do not truncate models or add a new message transport.
- Preserve the indexing of old persisted flat menus, including their default option and pagination. New provider-list menus use their persisted provider-local indices across restart and deduplication.
- Replace `/help` with the user's exact Markdown layout: bold title/section names, command on one line, description beneath, blank lines between entries, Tasks and Forwarding sections. No behavior change to commands.
- Update flow tests to choose a provider before a model. Test full lists including multi-message transport, saved-menu compatibility, final confirmation and safety guards; verify and obtain two independent reviews before safe activation.
