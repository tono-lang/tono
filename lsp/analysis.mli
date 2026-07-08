(* Editor-facing analysis derived entirely from the OCaml frontend. Pure (no IO,
   no transport) so every branch is unit-testable; the server module wires these
   answers to stdio JSON-RPC. *)

open Lsp.Types

(* Map a frontend span to an LSP range (1-based -> 0-based). *)
val range_of_span : Tono_frontend.Span.span -> Range.t

(* Byte offset of an LSP position inside the given document text. *)
val offset_of_position : string -> Position.t -> int

(* Parse source into the surface AST (parse-level only; diagnostics dropped). *)
val parse : string -> Tono_frontend.Ast.file

(* Full diagnostics (parse + lower + typecheck) for a document, as LSP values. *)
val lsp_diagnostics : string -> Diagnostic.t list

(* Hover for the node under the cursor: a declaration name or a type reference. *)
val hover_at :
  text:string -> file:Tono_frontend.Ast.file -> Position.t -> Hover.t option

(* Go-to-definition: a type reference under the cursor resolves to the declaring
   shape's name location in the same document. *)
val definition_at :
  uri:DocumentUri.t ->
  text:string ->
  file:Tono_frontend.Ast.file ->
  Position.t ->
  Location.t option

(* Completions: the file's declared shapes plus the primitive keywords. *)
val completions : file:Tono_frontend.Ast.file -> CompletionItem.t list

(* Find references: every use of the name under the cursor (declaration site
   included when [include_decl]). *)
val references_at :
  uri:DocumentUri.t ->
  text:string ->
  file:Tono_frontend.Ast.file ->
  include_decl:bool ->
  Position.t ->
  Location.t list

(* Rename the name under the cursor across the document (declaration and all
   references). Empty edit when the cursor is not on a name. *)
val rename_at :
  uri:DocumentUri.t ->
  text:string ->
  file:Tono_frontend.Ast.file ->
  new_name:string ->
  Position.t ->
  WorkspaceEdit.t

(* Document outline: top-level shapes with their members as children. *)
val document_symbols : file:Tono_frontend.Ast.file -> DocumentSymbol.t list

(* Whole-document formatting via the frontend's canonical printer (the `tono fmt`
   engine). None on a parse error. *)
val formatting : text:string -> TextEdit.t list option
