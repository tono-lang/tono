# tono

The Tono language compiler: reads `.tono` files and generates idiomatic SDKs in
multiple languages. Polyglot monorepo - OCaml frontend, Rust backend.

## Layout

- `frontend/` - OCaml: lexer, parser, typecheck, IR
- `lsp/`      - OCaml: language server (reuses the frontend)
- `backend/`  - Rust: codegen engine
- `cli/`      - Rust: `tono` binary
- `ir-schema/`- serialized IR contract

## Build

- OCaml (`frontend/`, `lsp/`): `dune build`
- Rust (`backend/`, `cli/`): `cargo build`

## Install

```sh
curl -fsSL https://tono-lang.github.io/tono/install.sh | sh
```

Or via Homebrew:

```sh
brew install tono-lang/tono/tono
```

Either way you get three binaries: `tono` (the CLI), `tono-frontend` (the
parser/typechecker it shells out to), and `tono-lsp` (the language server
editors launch).

Prebuilt archives for macOS (arm64/x86_64) and Linux (x86_64/arm64) are also
attached directly to each [release](https://github.com/tono-lang/tono/releases).

## Update

- Homebrew: `brew upgrade tono`
- Install script: re-run the curl command above
- Manual: download the latest release archive for your platform

## Organizing generated SDKs

`tono init` scaffolds a `tono.toml` with one `[target.<lang>]` block per
language you enable, each with its own `out` directory. `out` accepts any
path, so how you version and publish what lands there is up to you. Three
patterns cover most cases:

- **Single repo, multi-target (the `init` default).** Each target's `out`
  (e.g. `out/ts`, `out/rust`, `out/go`) lives in the same repo as the `.tono`
  spec. No extra infrastructure: each target publishes straight to its own
  registry (`npm publish`, `cargo publish`) from a tag-triggered CI job. Go
  supports this natively: a module in a subdirectory is published with a
  prefixed tag (`out/go/v1.2.3`), per the [Go modules
  reference](https://go.dev/ref/mod), no separate repo required.
- **One repo per language (opt-in, more mature).** Most SDK generators
  default here instead (a monorepo is treated as the advanced setup): each
  language gets its own idiomatic repo (`org/api-sdk-python`,
  `org/api-sdk-go`), while the `.tono` spec and `tono.toml` stay the single
  source of truth in one place. Getting there from the single-repo default is
  a read-only split per package (a `git subtree`/`splitsh`-style mirror),
  which is separate, opt-in tooling `init` does not set up on its own.
- **Fully separate repo, manual sync.** Always available for free, since a
  target's `out` can point anywhere, including a sibling clone or a
  submodule. Maximum flexibility, zero opinion from `tono`, entirely up to
  you to keep in sync.
