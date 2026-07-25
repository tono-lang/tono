(* Duplicate non-repeatable traits (TC0047): a second @doc/@http/... on one
   declaration or member is rejected instead of silently letting one win,
   closing the trailing-trait absorption footgun with a diagnostic. *)

val check_decls : Ast.decl list -> Diagnostic.t list
