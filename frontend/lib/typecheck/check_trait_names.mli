(* Traits nothing will read (TC0054): a bare trait outside the compiler's
   vocabulary is reported as inert, with the nearest known name when there is
   one, instead of passing silently into the IR. *)

val check_decls : Ast.decl list -> Diagnostic.t list
