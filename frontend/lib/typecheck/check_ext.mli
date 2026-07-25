(* Structural validation of extension declarations: the closed hook lifecycle,
   the hook-vs-contract signature rule, supported binding languages, and the
   at-least-one-binding requirement. The conformance gate lives in the
   generator, not here. *)

(* The closed hook lifecycle, exposed so tooling (hover, completion) names
   exactly the slots this checker enforces and can never grow its own copy. *)
val hook_slots : string list
val check_decls : Ast.decl list -> Diagnostic.t list
