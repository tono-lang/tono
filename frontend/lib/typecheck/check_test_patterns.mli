(* Lowering of expect patterns to the IR pattern language, against user shapes,
   the tono.errors taxonomy shapes, and the http.request shape. *)

(* The taxonomy category names: api, validation, decode, contract, config,
   transport. *)
val taxonomy_categories : string list
val pattern_span : Ast.test_pattern -> Span.span

(* Lower a ctor pattern against a user struct; [as_error] selects the
   declared-error reading over the output reading. *)
val lower_struct_pattern :
  Check_test_values.ctx ->
  refty:Check_test_values.ref_typer ->
  shape:string ->
  members:Ast.member list ->
  as_error:bool ->
  Ast.test_pattern_ctor ->
  Ir.test_pattern

(* Lower an [errors.<category> { ... }] pattern; [None] when the category is
   unknown (diagnosed). *)
val lower_taxonomy_pattern :
  Check_test_values.ctx ->
  refty:Check_test_values.ref_typer ->
  category:string ->
  Ast.test_pattern_ctor ->
  Ir.test_pattern option

(* Lower a nested pattern at a member position (equality or nested struct). *)
val lower_sub_pattern :
  Check_test_values.ctx ->
  refty:Check_test_values.ref_typer ->
  Ast.ty ->
  Ast.test_pattern ->
  Ir.test_pattern

(* Lower one element of a [.requests] list ([http.request { ... }]). *)
val lower_request_pattern :
  Check_test_values.ctx ->
  refty:Check_test_values.ref_typer ->
  Ast.test_pattern ->
  Ir.request_pattern option
