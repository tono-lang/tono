(* Resolve every type reference in a file against the symbol table plus the
   in-scope generic parameters of the enclosing shape. An unresolved name is a
   TC0001. Arity of a generic application is checked in a later phase. *)

(* Resolve a single surface type; [params] are the type-parameter names in scope
   of the enclosing shape. *)
val resolve_ty :
  params:string list -> tbl:Symtab.t -> Ast.ty -> Diagnostic.t list

(* Resolve every type reference inside one declaration. *)
val resolve_decl : Symtab.t -> Ast.decl -> Diagnostic.t list

(* Resolve every type reference across a whole file. *)
val resolve_decls : Symtab.t -> Ast.file -> Diagnostic.t list
