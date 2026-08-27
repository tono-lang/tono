(* Trait, field-reference, and call-expression parsing shared by the
   declaration parsers. *)

(* ref ::= "." name ("." name)*  -- the caller has seen the leading dot. *)
val parse_ref_path : Parser_state.t -> Ast.ref_path

(* ctor ::= name "{" (field ":" value, comma-separated)? "}"  -- the caller
   has already consumed [name] at [name_span]. Always returns [Ast.ACtor]. *)
val parse_ctor_arg : Parser_state.t -> string -> Span.span -> Ast.trait_arg

(* trait ::= "@" name ("::" name)* ("(" arg ("," arg)* ")")? *)
val parse_trait : Parser_state.t -> Ast.trait

(* Zero or more traits at the cursor, on any line; stops at the first
   non-"@" token. The traits written before the item they belong to. *)
val parse_leading_traits : Parser_state.t -> Ast.trait list

(* Zero or more traits continuing the current line; stops at the first
   non-"@" token or at a trait that opens a line of its own. *)
val parse_inline_traits : Parser_state.t -> Ast.trait list

(* The leading traits of a body item (member, op, case, variant): read when
   the cursor sits on "@", kept only if [starts_item] accepts the token after
   them, otherwise diagnosed as dangling ([what] names the item kind in the
   message) and dropped. *)
val parse_item_traits :
  Parser_state.t ->
  what:string ->
  starts_item:(Token.kind -> bool) ->
  Ast.trait list

(* match ::= "match" ref "{" (pattern "=>" value)* "}" *)
val parse_field_match : Parser_state.t -> Ast.field_match

(* "(" call_arg ("," call_arg)* ")"  -- the caller has not consumed '(' yet.
   Used for a language block's [call: "symbol"(args)] line, which has no
   "ns." prefix. *)
val parse_call_args : Parser_state.t -> Ast.call_arg list

(* "#(symbol)" "(" call_arg ("," call_arg)* ")"  -- the spelling has already
   been consumed at [symbol]/[symbol_span]; the cursor sits on "(". A nested
   call in an argument, or the method a [call:] line chains on the returned
   object. *)
val parse_nested_call : Parser_state.t -> string -> Span.span -> Ast.nested_call

(* call_expr ::= "." name "(" call_arg ("," call_arg)* ")"  -- [ns]/[ns_span]
   have already been consumed by the caller; the cursor sits on the '.'. *)
val parse_call_expr :
  Parser_state.t -> ns:string -> ns_span:Span.span -> Ast.call_expr
