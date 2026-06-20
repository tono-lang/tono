(* Cursor over the token stream plus a diagnostic sink. The parser peeks and
   advances through this; it accumulates diagnostics rather than raising. The
   token stream always ends with [Eof], so [peek] is always valid and [advance]
   never moves past it. *)

type t = {
  toks : Token.t array;
  mutable pos : int;
  mutable diags : Diagnostic.t list; (* accumulated in reverse *)
}

let create (toks : Token.t list) : t =
  { toks = Array.of_list toks; pos = 0; diags = [] }

let peek st = st.toks.(st.pos)

let advance st =
  let t = st.toks.(st.pos) in
  if t.Token.kind <> Token.Eof then st.pos <- st.pos + 1;
  t

let error st (span : Span.span) (message : string) =
  st.diags <- Diagnostic.error span message :: st.diags

let diagnostics st = List.rev st.diags

(* Consume the next token if its kind matches; otherwise diagnose (without
   consuming) so the caller can resynchronize. [what] describes the expectation
   for the message. *)
let expect st (kind : Token.kind) (what : string) : Token.t option =
  let t = peek st in
  if t.Token.kind = kind then (
    ignore (advance st);
    Some t)
  else (
    error st t.span
      (Printf.sprintf "expected %s, found %s" what
         (Token.describe t.Token.kind));
    None)
