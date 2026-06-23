(* Enum validation over the surface AST: value names and explicit int values are
   unique (TC0008) and the backing is consistent (TC0009). *)

(* Validate one enum's value list. *)
val check_enum : Ast.enum_case list -> Diagnostic.t list

(* Validate every enum in the file. *)
val check_decls : Ast.file -> Diagnostic.t list
