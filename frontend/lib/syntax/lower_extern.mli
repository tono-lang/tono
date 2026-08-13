(* Lowering for the FFI ext/extern surface. *)

(* An extern call expression: resolving [ns]/[fn] against a declared [ext]
   block is deferred (out of scope); this only carries the call structured.
   Used both by [Lower]'s own entry-field [ef_call] and by this module's
   [lower_ext_lib]. *)
val lower_call_expr : Ast.call_expr -> Ir.entry_call

(* Lower a full [ext <name> { ... }] declaration. [lower_type]/[lower_select]
   are threaded in from [Lower] to avoid a dependency cycle. *)
val lower_ext_lib :
  lower_type:
    (params:string list ->
    resolve:(qualifier:string option -> name:string -> Ir.shape_id) ->
    diags:Diagnostic.t list ref ->
    Ast.ty ->
    Ir.tref) ->
  lower_select:(diags:Diagnostic.t list ref -> Ast.field_match -> Ir.select) ->
  resolve:(qualifier:string option -> name:string -> Ir.shape_id) ->
  diags:Diagnostic.t list ref ->
  Ast.decl ->
  Ir.ext_lib
