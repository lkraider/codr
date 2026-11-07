# Source and release mirrors

[GitHub](https://github.com/tidley/codr) is primary for **codr** source, issues, CI, and releases.
Pull requests and their reviews are welcome on GitHub or ngit. Contributors choose one route; duplicate PRs are not required. GitHub contributors do not need Nostr or ngit, and ngit contributors do not need a GitHub account.
The [ngit repository](https://gitworkshop.dev/npub19at4nqjjymca2lh3v546vcg5ktun3al4c5xu2pts3ewckv0y70ys0gupzr/relay.ngit.dev/codr)
provides independent access to approved source and published release builds.
Blossom hosts the mirrored release files.

## Source refs

1. Review and verify the change.
2. Publish the approved branch or tag to GitHub.
3. Push that exact ref to the existing ngit remote.
4. Read both remote refs and compare their commit hashes.

Keep `master` stable and `dev/linux` separate. Do not merge preview code into stable to update a mirror.
Do not move published tags, force updates, delete unrelated refs, or use `git push --mirror`.
Keep existing remote names and URLs. This maintainer checkout uses `github` for GitHub and `origin` for ngit.

## Release builds

GitHub Actions builds and publishes the release first. Mirror published release assets, not new builds of the same source.
The mirror must retain the version, notes, source commit, release date, and stable or preview channel.
Keep Linux previews separate from stable releases. A mirror must not promote a preview to stable.

### Contributor credits

Published release notes must include a **Contributors** section covering the
release's changes from both GitHub and ngit. Use original commit authors,
co-authors and verified contribution records from either host; GitHub's generated
list alone is not sufficient. Preserve the contributor's original credit and do
not require a GitHub account. Combine identities only when the link is verified,
not from matching names. Keep these credits identical in mirrored notes.

### Asset verification

1. Read the GitHub release metadata and list every published asset.
2. Download the original archives and checksum files.
3. Verify each archive against its published SHA-256 checksum.
4. Check existing ngit release records and Blossom uploads before each attempt.
5. Reuse matching verified files and records. Stop if content or metadata conflicts.
6. Upload the exact files to Blossom. Do not rebuild, repackage, or re-sign them.
7. Publish the matching signed ngit release record.
8. Download every archive and checksum file from Blossom.
9. Compare each downloaded file's SHA-256 hash with the GitHub original.

A successful upload alone does not confirm a complete mirror.
Report missing files, checksum failures, incomplete metadata, or failed publication explicitly.
Keep original GitHub releases and assets unchanged.
The install hook still uses GitHub Releases. Independent downloads use the verified ngit release records and Blossom URLs.

## CI evidence

A matching source ref does not prove that CI passed.
Keep the original GitHub CI run links and their exact source commits.
Mirror existing CI evidence through the repository's supported ngit tooling.
If that tooling has no import path, report the CI evidence mirror as blocked.
Do not rebuild solely for mirroring, invent CI results, or create releases for test builds.

## Signing and collaboration

On Tom's DevBox, use the configured `devbox-amber` remote signer.
Tom must approve each signing request in Amber. Never export signing secrets or enable automatic approval.
An approval timeout or rejection is not a successful publication.
CI can finish before the mirror because publication needs this approval and download verification.

Keep existing ngit issues, PRs, comments, and attribution. Accept new PRs on either host; use GitHub for new issues. Integrate approved source through GitHub first, then mirror the approved refs without requiring duplicate PRs.
Do not duplicate discussions or close old records solely because the hosting policy changed.
See [Contributing](../CONTRIBUTING.md).
