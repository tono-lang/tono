(* Match-selection validation: subject scalarity, typed literal patterns,
   duplicate/unreachable arms, arm value resolution, and exhaustiveness
   (TC0038/TC0040/TC0041). *)

val check_match :
  Entry_scope.ctx ->
  Ast.member list ->
  Ast.member ->
  Ast.field_match ->
  Diagnostic.t list
