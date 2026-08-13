(* The legacy [ext hook|contract|constraint|impl] grammar. *)

(* Parse the kind word right after "ext" (hook/contract/constraint/impl). *)
val parse_ext_kind : Parser_state.t -> Ast.ext_kind * Span.span

(* Parse a full legacy [ext] declaration. The "ext" keyword is consumed here.
   [parse_type] is threaded in to avoid a dependency cycle with [Parser]. *)
val parse_ext :
  parse_type:(Parser_state.t -> Ast.ty) ->
  Parser_state.t ->
  pub:bool ->
  dtraits:Ast.trait list ->
  Ast.decl
