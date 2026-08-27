(* The type grammar, shared by every declaration parser. *)

(* type ::= base "?"?  -- parse a type expression from the cursor into a
   surface type. *)
val parse_type : Parser_state.t -> Ast.ty

(* generics ::= "[" name ("," name)* "]"  -- the type parameters declared
   after a struct or union name; [] when no '[' follows. *)
val parse_generics : Parser_state.t -> string list
