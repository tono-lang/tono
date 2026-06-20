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
