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

(* Stable sort by source position so combined lex/parse/lowering diagnostics read
   in source order; ties keep their original (phase) order. *)
let sort (ds : t list) : t list =
  let offset (d : t) = d.span.start.offset in
  List.stable_sort (fun a b -> Int.compare (offset a) (offset b)) ds
