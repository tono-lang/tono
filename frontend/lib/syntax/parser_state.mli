(* Cursor over the token stream with a diagnostic sink. *)

type t

val create : Token.t list -> t

(* The current token (never past [Eof]). *)
val peek : t -> Token.t

(* Return the current token and move forward (stops at [Eof]). *)
val advance : t -> Token.t

(* Whether the cursor is at the end-of-file token. *)
val at_eof : t -> bool

(* Record an error diagnostic at a span. *)
val error : t -> Span.span -> string -> unit

(* All diagnostics in source order. *)
val diagnostics : t -> Diagnostic.t list

(* Consume the next token if it matches [kind]; otherwise diagnose (describing
   [what]) without consuming, and return [None]. *)
val expect : t -> Token.kind -> string -> Token.t option
