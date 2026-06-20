(* Recursive-descent parser. More entry points (declarations, the public
   [parse]) are added as the grammar grows. *)

(* Parse a type expression from the cursor into a surface type. *)
val parse_type : Parser_state.t -> Ast.ty

(* Parse a trait (`@name(args)`) from the cursor. *)
val parse_trait : Parser_state.t -> Ast.trait

(* Parse a member (`name: type @trait*`) from the cursor. *)
val parse_member : Parser_state.t -> Ast.member

(* Parse a `struct` declaration; [pub] and [dtraits] are the visibility and
   shape-level traits already consumed before the keyword. *)
val parse_struct :
  Parser_state.t -> pub:bool -> dtraits:Ast.trait list -> Ast.decl
