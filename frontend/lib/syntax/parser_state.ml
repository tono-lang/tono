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

(* The token [n] positions past the cursor, clamped to the trailing [Eof]. *)
let peek_ahead st n =
  let i = st.pos + n in
  if i < Array.length st.toks then st.toks.(i)
  else st.toks.(Array.length st.toks - 1)

let advance st =
  let t = st.toks.(st.pos) in
  if t.Token.kind <> Token.Eof then st.pos <- st.pos + 1;
  t

let at_eof st = (peek st).Token.kind = Token.Eof

(* Whether the current token is the first one on its line. Line breaks are
   otherwise insignificant, but a trait on a line of its own belongs to the
   item after it while an inline one stays with its line, so this is the one
   place the parser looks at layout. Comments are not tokens, so a trait under
   a comment line still opens its own line. *)
let starts_line st =
  st.pos = 0
  || (peek st).Token.span.start.line
     > st.toks.(st.pos - 1).Token.span.finish.line

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
