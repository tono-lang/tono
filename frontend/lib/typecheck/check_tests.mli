(* Declared-test validation and lowering: resolves every binding reference
   backwards, types stub values, call inputs, and expect patterns against the
   declarations they fill, and lowers each test block to its IR encoding. *)

val check_decls :
  imports:Ast.import list ->
  Ast.decl list ->
  Ir.test_decl list * Diagnostic.t list
