(* Structural validation of extension declarations: hooks are rejected
   outright, the contract/impl signature rule, supported binding languages,
   and the at-least-one-binding requirement. The conformance gate lives in the
   generator, not here. *)

val check_decls : Ast.decl list -> Diagnostic.t list
