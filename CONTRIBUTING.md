# Contributing

[GitHub](https://github.com/tidley/codr) is the primary home for **codr** source, issues, CI, and releases.
Pull requests are welcome on **GitHub or ngit**. Choose either route; you do not need to submit the same PR to both.
The GitHub route does not require a Nostr account or ngit. The ngit route does not require a GitHub account.

## Clone and contribute

Fork the repository on GitHub, then clone your fork:

```sh
git clone https://github.com/YOUR-USER/codr.git
cd codr
git switch -c my-change
```

Run the relevant tests and commit your change. Push your branch to your fork, then open a GitHub pull request.
Use `master` for stable changes or `dev/linux` for Linux preview changes. State which branch your change targets.
For bug reports, use [GitHub Issues](https://github.com/tidley/codr/issues).

## Pull requests through ngit

Open a PR in the [ngit repository on GitWorkshop](https://gitworkshop.dev/npub19at4nqjjymca2lh3v546vcg5ktun3al4c5xu2pts3ewckv0y70ys0gupzr/relay.ngit.dev/codr), or use the [ngit CLI](https://ngit.dev/pull-requests).
Use your own Nostr signer. For the CLI branch-push workflow, create a branch whose name starts with `pr/`, such as `pr/my-change`; pushing a normal branch does not create a PR.
Target `master` for stable changes or `dev/linux` for Linux preview changes, as with GitHub PRs.

Maintainers can review submissions on either host. Preserve the original PR, discussion, contributor commits, and attribution when integrating approved work through GitHub, then mirror the approved refs to ngit.
Do not require an ngit contributor to open a duplicate GitHub PR.

## Maintainer mirror

GitHub is primary. Publish reviewed commits and tags to GitHub first, then mirror the same approved refs to ngit.
Identify remotes by URL. This maintainer checkout keeps `github` for `https://github.com/tidley/codr.git` and `origin` for the existing `nostr://` mirror.
Contributors can keep the normal GitHub `origin` name in their own clones.
Do not use `git push --mirror`: it can remove unrelated remote refs.

Keep `master` as the stable branch and `dev/linux` as the Linux preview branch.
Do not move release tags or replace published assets during mirror updates.

GitHub Actions builds releases. Mirror the exact published archives, checksum files, notes, source commit, release date, and stable or preview channel.
Do not rebuild, repackage, re-sign, or replace published assets to create a mirror.
Download each mirrored asset from Blossom and compare its SHA-256 hash with the GitHub original before reporting success.
See [mirror operations](docs/MIRRORING.md).

## Discussions on both hosts

Keep existing ngit issues, PRs, comments, and attribution. Do not duplicate or close them solely because GitHub is now primary.
New pull requests are welcome on either host; use GitHub for new issues. Move an existing discussion only with the author's or maintainer's agreement, and link its original record.

Juraj Bednar's (`jooray`) OpenCode executable contribution was integrated in `c64eaabc3f7e3e2b449a576208a4d9dda0b86b99`.
The [ngit carry-over record](https://gitworkshop.dev/nevent1qqsp9m5qz37u050larxt7vh7fruzcgjc38rclxxs3a4pp0dex08ud2cpz3mhxue69uhhyetvv9ujumn8d96zuer9wcvy8xg5)
preserves the original attribution and discussion.
