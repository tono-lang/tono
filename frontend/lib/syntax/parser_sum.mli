(* The sum declarations: union and enum. [pub] and [dtraits] are the
   visibility and shape-level traits already consumed before the keyword;
   the keyword itself is consumed here. *)

(* union ::= "union" name generics? "{" variant* "}" *)
val parse_union :
  Parser_state.t -> pub:bool -> dtraits:Ast.trait list -> Ast.decl

(* enum ::= "enum" name "{" case* "}" *)
val parse_enum :
  Parser_state.t -> pub:bool -> dtraits:Ast.trait list -> Ast.decl
