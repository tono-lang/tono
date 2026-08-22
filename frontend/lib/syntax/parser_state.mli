(* Cursor over the token stream with a diagnostic sink. *)

type t

val create : Token.t list -> t

(* The current token (never past [Eof]). *)
val peek : t -> Token.t

(* The token [n] positions past the cursor, clamped to the trailing [Eof]. *)
val peek_ahead : t -> int -> Token.t

(* Return the current token and move forward (stops at [Eof]). *)
val advance : t -> Token.t

(* Whether the cursor is at the end-of-file token. *)
val at_eof : t -> bool

(* Whether the current token is the first one on its line (or the first in
   the file). *)
val starts_line : t -> bool

(* Record an error diagnostic at a span. *)
val error : t -> Span.span -> string -> unit

(* All diagnostics in source order. *)
val diagnostics : t -> Diagnostic.t list

(* Consume the next token if it matches [kind]; otherwise diagnose (describing
   [what]) without consuming, and return [None]. *)
val expect : t -> Token.kind -> string -> Token.t option
