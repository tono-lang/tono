(* Trait, field-reference, and call-expression parsing shared by the
   declaration parsers. *)

(* ref ::= "." name ("." name)*  -- the caller has seen the leading dot. *)
val parse_ref_path : Parser_state.t -> Ast.ref_path

(* ctor ::= name "{" (field ":" value, comma-separated)? "}"  -- the caller
   has already consumed [name] at [name_span]. Always returns [Ast.ACtor]. *)
val parse_ctor_arg : Parser_state.t -> string -> Span.span -> Ast.trait_arg

(* trait ::= "@" name ("::" name)* ("(" arg ("," arg)* ")")? *)
val parse_trait : Parser_state.t -> Ast.trait

(* Zero or more traits at the cursor; stops at the first non-"@" token. *)
val parse_trailing_traits : Parser_state.t -> Ast.trait list

(* match ::= "match" ref "{" (pattern "=>" value)* "}" *)
val parse_field_match : Parser_state.t -> Ast.field_match

(* "(" call_arg ("," call_arg)* ")"  -- the caller has not consumed '(' yet.
   Used for a language block's [call: "symbol"(args)] line, which has no
   "ns." prefix. *)
val parse_call_args : Parser_state.t -> Ast.call_arg list

(* call_expr ::= "." name "(" call_arg ("," call_arg)* ")"  -- [ns]/[ns_span]
   have already been consumed by the caller; the cursor sits on the '.'. *)
val parse_call_expr :
  Parser_state.t -> ns:string -> ns_span:Span.span -> Ast.call_expr
