(* A known trait written where nothing reads it (TC0069). *)

(* The two position groups, exposed so a test can assert they are disjoint:
   a name in both would make the rule's answer depend on which branch of
   [illegal_at] happens to run first, silently misclassifying it. *)
val member_only : string list
val op_only : string list
val check_decls : Ast.decl list -> Diagnostic.t list
