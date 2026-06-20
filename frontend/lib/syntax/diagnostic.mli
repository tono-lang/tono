(* Structured, span-carrying diagnostics accumulated during lex/parse. *)

type severity = Error | Warning
type t = { span : Span.span; severity : severity; message : string }

val error : Span.span -> string -> t
val warning : Span.span -> string -> t
val severity_to_string : severity -> string
val to_string : t -> string
