# tree-sitter-tono

A Tree-sitter grammar for the tono interface language, for editor and GitHub
syntax highlighting (Neovim, Zed, Helix, and any Tree-sitter host).

This is intentionally a second, lightweight parser. The OCaml frontend remains
the single source of truth for parsing, typechecking, and diagnostics; this
grammar only colors a file and must stay tolerant of work in progress. Keep it in
sync with the surface syntax the frontend accepts.

## Layout

- `grammar.js` - the grammar (the source of truth here).
- `queries/highlights.scm` - highlight captures.
- `test/corpus/` - parse tests (`tree-sitter test`).

The parser under `src/` is generated and gitignored; regenerate it with the CLI.

## Develop

```sh
npm install            # provides tree-sitter-cli, or use a system install
tree-sitter generate   # grammar.js -> src/parser.c
tree-sitter test       # run the corpus
tree-sitter parse ../examples/payments/payments.tono   # sanity-check a real file
```
