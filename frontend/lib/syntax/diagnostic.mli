(* Structured, span-carrying diagnostics accumulated during lex/parse. *)

type severity = Error | Warning

type t = {
  span : Span.span;
  severity : severity;
  message : string;
  (* stable diagnostic code, e.g. "TC0001"; None for lex/parse *)
  code : string option;
}

val error : ?code:string -> Span.span -> string -> t
val warning : ?code:string -> Span.span -> string -> t
val severity_to_string : severity -> string
val to_string : t -> string

(* Stable sort by source position (offset), keeping ties in their original order. *)
val sort : t list -> t list
