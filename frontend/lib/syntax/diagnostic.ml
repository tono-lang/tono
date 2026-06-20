(* Structured, span-carrying diagnostics. The lexer and parser never raise on
   malformed input; they accumulate diagnostics and return them alongside a
   best-effort result. *)

type severity = Error | Warning
type t = { span : Span.span; severity : severity; message : string }

let error (span : Span.span) (message : string) : t =
  { span; severity = Error; message }

let warning (span : Span.span) (message : string) : t =
  { span; severity = Warning; message }

let severity_to_string = function Error -> "error" | Warning -> "warning"

let to_string (d : t) : string =
  Printf.sprintf "%s: %s: %s" (Span.to_string d.span)
    (severity_to_string d.severity)
    d.message
