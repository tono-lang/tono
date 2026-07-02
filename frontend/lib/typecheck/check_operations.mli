(* Operation error references and error-discrimination traits: each @errors
   entry must resolve to a declared shape (TC0014) carrying a valid @status
   (TC0015) and a well-formed optional @errorCode (TC0016); the (status, code)
   pair must be unique within the operation (TC0017), and @async takes no
   arguments (TC0018). Inputs and outputs are resolved as ordinary references. *)

(* Check every operation's @errors entries and effect trait. *)
val check_decls : Symtab.t -> Ast.file -> Diagnostic.t list
