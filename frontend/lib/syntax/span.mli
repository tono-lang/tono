(* Source positions and spans, carried on every token and AST node. *)

type pos = { line : int; col : int; offset : int }
type span = { start : pos; finish : pos }

(* [merge a b] spans from the start of [a] to the end of [b]. *)
val merge : span -> span -> span

(* A compact "line:col-col" / "line:col-line:col" rendering for messages. *)
val to_string : span -> string
