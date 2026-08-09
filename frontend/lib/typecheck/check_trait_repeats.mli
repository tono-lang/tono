(* Duplicate non-repeatable traits (TC0047): a second @doc/@http/... on one
   declaration or member is rejected instead of silently letting one win,
   closing the trailing-trait absorption footgun with a diagnostic. *)

(* Names that mean one thing per declaration or member. Repeatability is not
   a [Trait_vocab] group property (it cuts across groups), so this stays its
   own list; exposed so a test can at least check it names nothing outside
   the vocabulary. *)
val non_repeatable : string list
val check_decls : Ast.decl list -> Diagnostic.t list
