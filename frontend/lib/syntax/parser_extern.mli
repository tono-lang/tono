(* The new "ext <name> { ... }" FFI library-block grammar. *)

(* Diagnoses a bare 'error' head before delegating to [parse_type]; used at
   the two positions where 'error' would otherwise be read as an ordinary
   type name (an extern's return type, a [returns:] type). *)
val parse_type_no_error :
  parse_type:(Parser_state.t -> Ast.ty) ->
  Parser_state.t ->
  ctx:string ->
  Ast.ty

(* Parse the body and close of "ext <name> { ... }"; "ext" and [name] have
   already been consumed by the caller (see [Parser]'s disambiguation at
   "ext <ident>"). *)
val parse_ext_lib :
  parse_type:(Parser_state.t -> Ast.ty) ->
  parse_type_no_error:(Parser_state.t -> ctx:string -> Ast.ty) ->
  Parser_state.t ->
  pub:bool ->
  dtraits:Ast.trait list ->
  name:string ->
  name_span:Span.span ->
  Ast.decl
