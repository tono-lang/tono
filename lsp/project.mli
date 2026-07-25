(* The request layer built on [Analysis]: workspace projects, cross-file
   navigation, workspace rename and symbols, quick fixes, and signature help.
   Pure; the server feeds file contents and round-trips document ids (URIs). *)

open Lsp.Types

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
