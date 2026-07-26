(* Operation-position rules of the entry model (TC0038/TC0043/TC0044):
   endpoint refs, typed @timeout/@retry, @header key/value forms, template
   spans, and the entry-only surface rejected on loose operations. *)

val check_loose_op : Ast.decl -> Diagnostic.t list

val check_entry_op :
  Entry_scope.ctx -> Ast.member list -> Ast.decl -> Diagnostic.t list
