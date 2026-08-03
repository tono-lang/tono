# Per-language mirror repositories

By default a tono project is a monorepo: the `.tono` spec and every generated
SDK live together, and nothing in this page applies. When a project outgrows
that (each language community expects `org/api-sdk-go`, `org/api-sdk-python`,
and so on as a dedicated repository), `tono split` mirrors each target's `out`
directory into its own repository at release time. The monorepo stays the
single source of truth: development, review, and tagging happen there only,
and each mirror is a read-only projection rebuilt from the monorepo's history.

## Prerequisites

- **The mirror repository must already exist.** `tono split` pushes to it but
  never creates it; creating repositories touches org permissions and billing,
  which stay in your hands. Create an empty repository (no README, no initial
  commit) per target before configuring it.
- **Push credentials in the environment.** The push is plain `git push`; use
  a per-repository deploy key with write access, or a token wired through
  `git config url.insteadOf` (example below).
- **Full history.** The subtree is rebuilt from history, so a shallow clone is
  refused. In GitHub Actions, check out with `fetch-depth: 0`.
- **Committed output.** The mirror is a projection of what the monorepo has
  committed under `out`, so generated SDKs must be committed there.

## Configuration

Mirroring is opt-in per target. A target without `split_repo` keeps its
monorepo-only behavior, whatever the other targets do:

```toml
[target.go]
out        = "out/go"
split_repo = "acme/payments-go"        # GitHub shorthand

[target.typescript]
out        = "out/ts"
split_repo = "git@github.com:acme/payments-ts.git"  # any git URL works

[target.rust]
out = "out/rust"                        # no split_repo: monorepo only
```

The `owner/name` shorthand expands to `https://github.com/owner/name.git`;
anything naming a protocol, host, or path is passed to git verbatim.

## The command

```
tono split --branch <name> [--config <tono.toml>] [--ref <committish>]
```

For each target with `split_repo`, it runs `git subtree split` over the
target's `out` prefix at `--ref` and force-pushes the projected head to the
mirror branch named by `--branch`.

`--branch` is required: the command never invents a destination, the caller
always says where the changes land. `--ref` defaults to the repository's
default branch, resolved the way git itself defines it (the `origin` remote's
`HEAD`): the locally recorded `origin/HEAD` symref when the checkout came
from a plain clone, otherwise the remote is asked directly, so it also works
in CI checkouts that were built by fetch. The remote-tracking ref is used,
not a possibly stale local branch. With no usable `origin`, the command asks
for an explicit `--ref`.

The release flow stays yours, and so does versioning: `tono split` moves a
projection to a branch, nothing more. It never tags the mirror; if your
release resolves versions from mirror tags (Go modules in particular), tag it
as a step of your own process, for example with the projected head the
command prints:

```
git push https://github.com/acme/payments-go.git <sha>:refs/tags/v1.2.0
```

Nothing requires release ceremony either: to cut a one-off prerelease for a
single client from any monorepo branch, aim it at its own mirror branch and
the mirror's `main` stays untouched:

```
tono split --ref feat/acme-pilot --branch alpha-acme
```

Targets are split independently, best effort: a mirror that cannot be pushed
(missing repository, revoked credential) is reported and the remaining targets
still run. The command exits non-zero if any target failed, after attempting
all of them.

The split is deterministic: re-running over the same history produces the same
commits, so consecutive releases extend the mirror's history instead of
rewriting it. The force-push exists so the projection is always authoritative;
mirrors are read-only, and pull requests against them are not a supported
flow. Point contributors at the monorepo.

## Release workflow

Trigger the split from the same tag that cuts a release, after publishing:

```yaml
name: Mirror SDKs

on:
  push:
    tags:
      - "v*"

jobs:
  split:
    runs-on: ubuntu-latest
    permissions:
      contents: read
    steps:
      - uses: actions/checkout@v5
        with:
          fetch-depth: 0   # the subtree is rebuilt from full history

      # SDK_MIRROR_TOKEN: a PAT (or GitHub App token) with write access to
      # every mirror repository. GITHUB_TOKEN cannot push to other repos.
      - name: Authenticate mirror pushes
        run: |
          git config --global \
            url."https://x-access-token:${{ secrets.SDK_MIRROR_TOKEN }}@github.com/".insteadOf \
            "https://github.com/"

      - name: Install tono
        run: curl -fsSL https://tono-lang.github.io/tono/install.sh | sh

      - name: Mirror the tagged SDKs
        run: tono split --ref "$GITHUB_REF_NAME" --branch main
```
