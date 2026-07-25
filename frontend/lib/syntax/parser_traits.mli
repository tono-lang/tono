(* Trait and field-reference parsing shared by the declaration parsers. *)

(* ref ::= "." name ("." name)*  -- the caller has seen the leading dot. *)
val parse_ref_path : Parser_state.t -> Ast.ref_path

(* trait ::= "@" name ("::" name)* ("(" arg ("," arg)* ")")? *)
val parse_trait : Parser_state.t -> Ast.trait

(* Zero or more traits at the cursor; stops at the first non-"@" token. *)
val parse_trailing_traits : Parser_state.t -> Ast.trait list
