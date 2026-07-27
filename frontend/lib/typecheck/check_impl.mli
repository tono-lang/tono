(* The implementation count: every operation is implemented exactly once, by a
   protocol binding (@http) or by a bespoke "ext impl", and an impl names an
   operation that exists unambiguously in its module. *)
val check_decls : Ast.decl list -> Diagnostic.t list
