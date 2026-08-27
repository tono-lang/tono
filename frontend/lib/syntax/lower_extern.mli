(* Lowering for the FFI ext/extern surface. *)

(* An extern call expression: resolving [ns]/[fn] against a declared [ext]
   block is deferred (out of scope); this only carries the call structured.
   Used both by [Lower]'s own entry-field [ef_call] and by this module's
   [lower_ext_lib]. *)
val lower_call_expr : Ast.call_expr -> Ir.entry_call

(* One call argument, shared by [lower_call_expr] and [Lower]'s own op
   [impl .field.method(args)] lowering (whose receiver is a field path, not
   an "ext" namespace, so it cannot reuse [lower_call_expr] itself).
   [classes] are the names a bare argument may pass as a class reference
   (the block's handles and the module's wire structs). *)
val lower_call_arg :
  ?classes:string list -> ?params:string list -> Ast.call_arg -> Ir.call_arg

(* The language blocks of a top-level (error) struct, as the "foreign"
   trait of its shape; [] when it has none. *)
val foreign_trait : Ast.lang_block list -> Ir.trait list

(* Lower a full [ext <name> { ... }] declaration. [lower_type]/[lower_select]
   are threaded in from [Lower] to avoid a dependency cycle; [structs] are
   the module's structs a call: line may pass as a class reference
   ([Roles.class_structs]). *)
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
  structs:string list ->
  Ast.decl ->
  Ir.ext_lib
