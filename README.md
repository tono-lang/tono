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
