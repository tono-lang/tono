(* Editor-facing analysis derived entirely from the OCaml frontend. Pure (no IO,
   no transport) so every branch is unit-testable; the server module wires these
   answers to stdio JSON-RPC. *)

open Lsp.Types

(* Map a frontend span (1-based, byte columns) to an LSP range (0-based, UTF-16
   code units). The document text is needed to decode the line's UTF-8. *)
val range_of_span : text:string -> Tono_frontend.Span.span -> Range.t

(* Byte offset of an LSP position (UTF-16 character column) inside the given
   document text. *)
val offset_of_position : string -> Position.t -> int

(* Parse source into the surface AST (parse-level only; diagnostics dropped). *)
val parse : string -> Tono_frontend.Ast.file

(* Whether the string is a single valid identifier per the frontend lexer
   (keywords and primitive names are not). *)
val valid_identifier : string -> bool

(* Full diagnostics (parse + lower + typecheck) for a document, as LSP values. *)
val lsp_diagnostics : string -> Diagnostic.t list

(* Hover for the node under the cursor, most specific first: a declaration
   name or a member name (the declaration or member pretty-printed by the
   canonical fmt printer plus its @doc prose), a type reference (the full
   target declaration), a trait (its contract), then keywords, primitives, and
   the ? marker. [markdown] selects a fenced tono code block or the plaintext
   fallback for clients without markdown support. *)
val hover_at :
  ?foreign:Foreign_index.lookup ->
  markdown:bool ->
  text:string ->
  file:Tono_frontend.Ast.file ->
  Position.t ->
  Hover.t option

(* Go-to-definition: a type reference under the cursor resolves to the declaring
   shape's name location in the same document. *)
val definition_at :
  uri:DocumentUri.t ->
  text:string ->
  file:Tono_frontend.Ast.file ->
  Position.t ->
  Location.t option

(* Context-aware completions: traits after `@`, lifecycle slots after
   `ext hook`, primitives plus declared shapes in type position, and the flat
   list of shapes and primitives elsewhere. *)
val completions :
  ?foreign:Foreign_index.lookup ->
  text:string ->
  file:Tono_frontend.Ast.file ->
  Position.t ->
  CompletionItem.t list

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
val document_symbols :
  text:string -> file:Tono_frontend.Ast.file -> DocumentSymbol.t list

(* Whole-document formatting via the frontend's canonical printer (the `tono fmt`
   engine). None on a parse error. *)
val formatting : text:string -> TextEdit.t list option

(* A range for a span inside the given text (UTF-16 columns). *)
val range_in : text:string -> Tono_frontend.Span.span -> Range.t

(* --- internals shared with the request layer (Project) --- *)

val contains : Tono_frontend.Span.span -> int -> bool

val name_sub_span :
  Tono_frontend.Span.span -> skip:int -> len:int -> Tono_frontend.Span.span

val decl_tys : Tono_frontend.Ast.decl -> Tono_frontend.Ast.ty list

val find_decl :
  Tono_frontend.Ast.file -> string -> Tono_frontend.Ast.decl option

val decl_symbol_kind : Tono_frontend.Ast.decl -> SymbolKind.t

val lsp_of_fdiags :
  text:string -> Tono_frontend.Diagnostic.t list -> Diagnostic.t list

val file_traits : Tono_frontend.Ast.file -> Tono_frontend.Ast.trait list
