(* Operation error references: each @errors entry must resolve to a declared
   shape (TC0014). Inputs and outputs are resolved as ordinary references. *)

(* Check every operation's @errors entries against the symbol table. *)
val check_decls : Symtab.t -> Ast.file -> Diagnostic.t list
