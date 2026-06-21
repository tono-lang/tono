(* Source positions and spans. Every token and AST node carries a span so
   diagnostics, and later the LSP, can point at precise ranges. *)

type pos = { line : int; col : int; offset : int }
type span = { start : pos; finish : pos }

(* A span covering both, from the start of [a] to the end of [b]. *)
let merge (a : span) (b : span) : span = { start = a.start; finish = b.finish }

let to_string (s : span) : string =
  if s.start.line = s.finish.line then
    Printf.sprintf "%d:%d-%d" s.start.line s.start.col s.finish.col
  else
    Printf.sprintf "%d:%d-%d:%d" s.start.line s.start.col s.finish.line
      s.finish.col
