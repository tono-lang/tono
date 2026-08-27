(* The pure side of foreign-binding diagnostics: which (ext, language) pairs
   a file declares, what each pair's verdict depends on, and how the JSON
   report of `tono check` becomes LSP diagnostics. See binding_check.ml. *)

open Lsp.Types

type pair = { ext : string; lang : string }

(* [typescript] and [ts] are one language to the check. *)
val normalize_lang : string -> string

(* The pairs the file declares, in order of first appearance, each with the
   spans of the text that belongs to that language alone (the path line,
   the storage and form blocks, the [call:] bodies). *)
val regions :
  Tono_frontend.Ast.file -> (pair * Tono_frontend.Span.span list) list

val pairs : Tono_frontend.Ast.file -> pair list

(* A digest of everything the pair's verdict depends on: the file with every
   language region removed (what every language crosses), the pair's own
   regions, and the manifest tables for the ext and the target. *)
val key :
  text:string ->
  manifest:string option ->
  Tono_frontend.Ast.file ->
  pair ->
  string

(* The pair's path line (else the ext's name): where a note about the whole
   pair is shown. *)
val anchor : Tono_frontend.Ast.file -> pair -> Tono_frontend.Span.span option

(* The check's JSON report lines for [pair] as diagnostics over [text]: a
   finding is an error re-located at its binding in [file], an unchecked
   note is information at the anchor, a check that could not run is a
   warning there, a passed line is nothing, and an unreadable line is a
   warning quoting it. *)
val diagnostics_of_lines :
  text:string ->
  file:Tono_frontend.Ast.file ->
  pair ->
  string list ->
  Diagnostic.t list

(* The report line a diagnostic was made from, rendered the way `tono
   check` prints it (None for a diagnostic not made from one). *)
val report_line : Diagnostic.t -> string option

(* Exposed for tests. *)
val without : text:string -> Tono_frontend.Span.span list -> string
val section : manifest:string -> string -> string
val span_of_string : text:string -> string -> Tono_frontend.Span.span option
