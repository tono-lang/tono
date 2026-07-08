(* Member-level validation over the surface AST. A member is [required] unless
   written [T?]; [@required] on a [T?] member is a TC0007 conflict. *)

(* Check a single struct member for nullability conflicts. *)
val check_member : Ast.member -> Diagnostic.t list

(* Check every struct member in the module. *)
val check_decls : Ast.decl list -> Diagnostic.t list
