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

(* Full diagnostics (parse + lower + typecheck) for a document, as LSP values. *)
val lsp_diagnostics : string -> Diagnostic.t list

(* Hover for the node under the cursor, most specific first: a declaration
   name or a member name (the declaration or member pretty-printed by the
   canonical fmt printer plus its @doc prose), a type reference (the full
   target declaration), a trait (its contract), then keywords, primitives, and
   the ? marker. [markdown] selects a fenced tono code block or the plaintext
   fallback for clients without markdown support. *)
val hover_at :
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

(* --- workspace projects --- *)

(* One file of a project: its dotted module name, an opaque document id the
   server round-trips (a URI string), and the parsed source. *)
type project_entry

val project_entry : module_:string -> id:string -> text:string -> project_entry

type project

(* Build the project index over every entry, through the frontend's own module
   machinery (the same resolution the CLI's compile-dir runs). *)
val build_project : project_entry list -> project

(* Diagnostics for one module with full project context: imports that resolve
   produce no false errors. *)
val project_diagnostics : project -> module_:string -> Diagnostic.t list

(* A content key over the module and its transitive imports: equal keys mean
   an identical check result, the caller's cache unit. *)
val module_check_key : project -> module_:string -> string

(* The declared symbol under the cursor as (declaring module, name), following
   qualified references through the imports. *)
val project_symbol_at :
  project -> module_:string -> Position.t -> (string * string) option

(* The declaration site of (module, name) as (document id, range). *)
val project_decl_location :
  project -> module_:string -> name:string -> (string * Range.t) option

(* Every reference to (module, name) across the project, as
   (document id, document text, span). *)
val project_occurrences :
  project ->
  module_:string ->
  name:string ->
  include_decl:bool ->
  (string * string * Tono_frontend.Span.span) list

val range_in : text:string -> Tono_frontend.Span.span -> Range.t

type rename_outcome =
  | Renamed of (string * TextEdit.t list) list
  | Collision of string
  | NotASymbol

(* Workspace rename: per-document edit lists, or a refusal when the new name
   collides with an existing declaration in the target module. *)
val project_rename :
  project -> module_:string -> Position.t -> new_name:string -> rename_outcome

(* Project-wide symbol search (case-insensitive substring) as
   (name, kind, document id, range). *)
val project_symbols :
  project -> query:string -> (string * SymbolKind.t * string * Range.t) list

(* Quick fixes for the diagnostics overlapping [range], keyed on diagnostic
   codes: add-import for an unknown module qualifier, the closest lifecycle
   slot for an unknown hook slot. (title, per-document edits). *)
val project_code_actions :
  project ->
  module_:string ->
  range:Range.t ->
  (string * (string * TextEdit.t) list) list

(* Signature help inside a trait's argument list, from the same registry as
   trait hover. *)
val signature_help :
  text:string ->
  file:Tono_frontend.Ast.file ->
  Position.t ->
  SignatureHelp.t option
