(* The "ext <name> { ... }" FFI library-block grammar: per-language module
   paths, foreign struct/opaque-handle declarations, and [op] declarations
   with a per-language call/yields/returns binding. Distinct from the legacy
   [ext hook|contract|constraint|impl] grammar in [Parser_ext], which this
   module does not touch. [parse_type] is threaded in (as in [Parser_ext])
   to avoid a dependency cycle with [Parser].

   The block has no contextual word of its own beyond the three lines of a
   language block: a declaration is "struct" or "op", a language block is
   "<lang> { ... }", and everything a target needs is a foreign spelling
   [#(...)] inside that target's block. *)

module P = Parser_state

(* [error] is reserved as a yields-position sentinel only; used as an
   ordinary type elsewhere (an op's return type, a [returns:] type) is a
   syntax error with a span. Everywhere else in the grammar (an ordinary
   field/param type) [error] still parses as a plain type name, unchanged. *)
let parse_type_no_error ~parse_type st ~(ctx : string) : Ast.ty =
  (match (P.peek st).kind with
  | Token.Ident "error" ->
      P.error st (P.peek st).span
        (Printf.sprintf
           "'error' is reserved for a yields position; not valid as %s" ctx)
  | _ -> ());
  parse_type st

(* A foreign struct or opaque type named [error] would collide with the
   yields-position sentinel: it could be declared but never referenced from
   an op's return type or a [returns:] type (both go through
   [parse_type_no_error] above), and a [yields:] entry naming it would be
   read as the sentinel instead of a reference to the shape. Reject the name
   at the declaration site instead, purely syntactic (no cross-referencing),
   so the collision is a clear error up front rather than a shape that
   quietly can never be used. *)
let check_not_error_name st (kind : string) (name : string) (span : Span.span) :
    unit =
  if String.equal name "error" then
    P.error st span
      (Printf.sprintf
         "'error' is reserved for a yields position; a %s cannot be named \
          'error'"
         kind)

let expect_ident st (what : string) : string * Span.span =
  let t = P.peek st in
  match t.kind with
  | Token.Ident n ->
      ignore (P.advance st);
      (n, t.span)
  | _ ->
      P.error st t.span ("expected " ^ what);
      ("", t.span)

let expect_foreign st (what : string) : string * Span.span =
  let t = P.peek st in
  match t.kind with
  | Token.Foreign s ->
      ignore (P.advance st);
      (s, t.span)
  | _ ->
      P.error st t.span ("expected a foreign spelling '#(...)' " ^ what);
      ("", t.span)

(* lang_block ::= lang "{" "#(...)" (name ":" "#(...)")* "}" -- the cursor
   sits on the language identifier. The first element is positional and
   names the foreign thing; the keyed entries name a tono field and give
   its foreign spelling. Shared by the ext header (where only the head is
   meaningful: the module path), a struct inside the block, and an error
   struct at top level. *)
let parse_lang_block st : Ast.lang_block =
  let lang, lang_span = expect_ident st "a language identifier" in
  ignore (P.expect st Token.LBrace "'{' to open the language block");
  let head, head_span =
    expect_foreign st "as the first element of a language block"
  in
  let rec fields acc =
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof -> List.rev acc
    | Token.Comma ->
        ignore (P.advance st);
        fields acc
    | Token.Ident _ ->
        let name, name_span = expect_ident st "a field name" in
        ignore (P.expect st Token.Colon "':' after a field name");
        let sp, sp_span = expect_foreign st "as the field's foreign form" in
        fields ((name, name_span, sp, sp_span) :: acc)
    | _ ->
        P.error st (P.peek st).span
          "unexpected token in a language block: expected 'field: #(...)'";
        ignore (P.advance st);
        fields acc
  in
  let flds = fields [] in
  let close = P.expect st Token.RBrace "'}' to close the language block" in
  {
    Ast.lb_lang = lang;
    lb_lang_span = lang_span;
    lb_head = head;
    lb_head_span = head_span;
    lb_fields = flds;
    lb_span =
      Span.merge lang_span
        (match close with Some t -> t.span | None -> head_span);
  }

(* An error struct's language block, at top level: "lang {" is the one item
   of a struct body a bare identifier opens with a brace (a member is always
   "name: type"), and it takes no traits of its own. *)
let parse_struct_lang_block st ~(traits : Ast.trait list) : Ast.lang_block =
  List.iter
    (fun (t : Ast.trait) ->
      P.error st t.Ast.tspan
        "a language block takes no traits; they belong to the struct")
    traits;
  parse_lang_block st

(* The ext header's module path: a language block with only the head. *)
let parse_lang_path st : Ast.lang_path =
  let b = parse_lang_block st in
  (match b.Ast.lb_fields with
  | [] -> ()
  | (_, span, _, _) :: _ ->
      P.error st span
        "the ext header's language block names only the module path; a field \
         spelling belongs to a struct's own block");
  {
    Ast.lp_lang = b.Ast.lb_lang;
    lp_lang_span = b.Ast.lb_lang_span;
    lp_path = b.Ast.lb_head;
    lp_path_span = b.Ast.lb_head_span;
  }

(* foreign_field ::= name ":" type *)
let parse_foreign_field ~parse_type st : Ast.foreign_field =
  let name, name_span = expect_ident st "a foreign field name" in
  ignore (P.expect st Token.Colon "':' after a foreign field name");
  { Ast.ff_name = name; ff_name_span = name_span; ff_type = parse_type st }

(* yields ::= "yields" ":" "(" (name ":" (type | "error" | "#(...)"), ",")+ ")" *)
let parse_yields_ty ~parse_type st : Ast.yields_ty =
  match (P.peek st).kind with
  | Token.Ident "error" ->
      let t = P.advance st in
      Ast.YError t.span
  | Token.Foreign s ->
      let t = P.advance st in
      Ast.YForeign (s, t.span)
  | _ -> Ast.YType (parse_type st)

let parse_yields ~parse_type st : Ast.yields_pos list =
  ignore (P.advance st);
  (* 'yields' *)
  ignore (P.expect st Token.Colon "':' after 'yields'");
  ignore (P.expect st Token.LParen "'(' to open the yields list");
  let one () =
    let name, name_span = expect_ident st "a yields name" in
    ignore (P.expect st Token.Colon "':' after a yields name");
    {
      Ast.yp_name = name;
      yp_name_span = name_span;
      yp_ty = parse_yields_ty ~parse_type st;
    }
  in
  let positions =
    match (P.peek st).kind with
    | Token.RParen ->
        P.error st (P.peek st).span "'yields:' must name at least one binding";
        []
    | _ ->
        let first = one () in
        let rec more acc =
          match (P.peek st).kind with
          | Token.Comma ->
              ignore (P.advance st);
              if (P.peek st).kind = Token.RParen then List.rev acc
              else more (one () :: acc)
          | _ -> List.rev acc
        in
        more [ first ]
  in
  ignore (P.expect st Token.RParen "')' to close yields");
  positions

(* returns_value ::= ref | match *)
let parse_returns_value st : Ast.returns_value =
  match (P.peek st).kind with
  | Token.Ident "match" -> Ast.RvMatch (Parser_traits.parse_field_match st)
  | Token.Dot -> Ast.RvRef (Parser_traits.parse_ref_path st)
  | _ ->
      P.error st (P.peek st).span
        "expected '.path' or 'match' as a returns field value";
      Ast.RvRef { Ast.segs = []; index = None; ref_span = (P.peek st).span }

let parse_returns_field st : Ast.returns_field =
  let name, name_span = expect_ident st "a returns field name" in
  ignore (P.expect st Token.Colon "':' after a returns field name");
  let value = parse_returns_value st in
  {
    Ast.rf_name = name;
    rf_name_span = name_span;
    rf_value = value;
    rf_span = name_span;
  }

(* returns ::= "returns" ":" type "{" (field ",")* "}" *)
let parse_returns ~parse_type_no_error st : Ast.returns_lit =
  let kw = P.advance st in
  (* 'returns' *)
  ignore (P.expect st Token.Colon "':' after 'returns'");
  let ty = parse_type_no_error st ~ctx:"a returns type" in
  ignore (P.expect st Token.LBrace "'{' to open the returns body");
  let rec fields acc =
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof -> List.rev acc
    | Token.Comma ->
        ignore (P.advance st);
        fields acc
    | Token.Ident _ -> fields (parse_returns_field st :: acc)
    | _ ->
        P.error st (P.peek st).span "unexpected token in returns body";
        ignore (P.advance st);
        fields acc
  in
  let flds = fields [] in
  let close = P.expect st Token.RBrace "'}' to close the returns body" in
  {
    Ast.rl_type = ty;
    rl_fields = flds;
    rl_span =
      Span.merge kw.span (match close with Some t -> t.span | None -> kw.span);
  }

(* call ::= "call" ":" "#(...)" "(" call_arg ("," call_arg)* ")" -- the
   callee is one foreign spelling, verbatim: what it is (a function, a
   class under `new`, a static method on a type) is the target's business. *)
let parse_call_line st : string * Span.span * Ast.call_arg list =
  ignore (P.advance st);
  (* 'call' *)
  ignore (P.expect st Token.Colon "':' after 'call'");
  let symbol, symbol_span = expect_foreign st "as the callee after 'call:'" in
  let args = Parser_traits.parse_call_args st in
  (symbol, symbol_span, args)

(* lang_body ::= lang "{" ("call:" | "yields:" | "returns:")* "}"
   -- "call:" is required; its absence is diagnosed but the rest still parses. *)
let parse_extern_lang_body ~parse_type ~parse_type_no_error st :
    Ast.extern_lang_body =
  let lang, lang_span = expect_ident st "a language identifier" in
  ignore (P.expect st Token.LBrace "'{' to open the language block");
  let call = ref None in
  let yields = ref None in
  let returns = ref None in
  let rec go () =
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof -> ()
    | Token.Comma ->
        ignore (P.advance st);
        go ()
    | Token.Ident "call" ->
        call := Some (parse_call_line st);
        go ()
    | Token.Ident "yields" ->
        yields := Some (parse_yields ~parse_type st);
        go ()
    | Token.Ident "returns" ->
        returns := Some (parse_returns ~parse_type_no_error st);
        go ()
    | _ ->
        P.error st (P.peek st).span
          (Printf.sprintf "unexpected token in a language block: expected %s"
             (Ext_lib_vocab.quoted Ext_lib_vocab.lang_fields));
        ignore (P.advance st);
        go ()
  in
  go ();
  let close = P.expect st Token.RBrace "'}' to close the language block" in
  let symbol, symbol_span, args =
    match !call with
    | Some c -> c
    | None ->
        P.error st lang_span "a language block requires a 'call:' line";
        ("", lang_span, [])
  in
  {
    Ast.elb_lang = lang;
    elb_lang_span = lang_span;
    elb_call_symbol = symbol;
    elb_call_symbol_span = symbol_span;
    elb_call_args = args;
    elb_yields = !yields;
    elb_returns = !returns;
    elb_span =
      Span.merge lang_span
        (match close with Some t -> t.span | None -> lang_span);
  }

(* op_param ::= name ":" type *)
let parse_extern_params ~parse_type st : Ast.extern_param list =
  ignore (P.expect st Token.LParen "'(' after the op name");
  let one () =
    let name, name_span = expect_ident st "a parameter name" in
    ignore (P.expect st Token.Colon "':' after a parameter name");
    { Ast.ep_name = name; ep_name_span = name_span; ep_type = parse_type st }
  in
  match (P.peek st).kind with
  | Token.RParen ->
      ignore (P.advance st);
      []
  | _ ->
      let first = one () in
      let rec more acc =
        match (P.peek st).kind with
        | Token.Comma ->
            ignore (P.advance st);
            more (one () :: acc)
        | _ -> List.rev acc
      in
      let ps = more [ first ] in
      ignore (P.expect st Token.RParen "')' to close op parameters");
      ps

(* op ::= "op" name "(" params ")" ":" type "{" lang_body+ "}" -- used both
   as a free declaration and as a handle's method; the cursor sits on "op",
   the traits written above it are passed in. *)
let parse_extern ~parse_type ~parse_type_no_error st ~(traits : Ast.trait list)
    : Ast.extern_decl =
  let kw = P.advance st in
  (* 'op' *)
  let name, name_span = expect_ident st "an op name" in
  let params = parse_extern_params ~parse_type st in
  ignore
    (P.expect st Token.Colon
       "':' after op parameters (a return type is required)");
  let ret = parse_type_no_error st ~ctx:"an op return type" in
  ignore (P.expect st Token.LBrace "'{' to open the op body");
  let rec langs acc =
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof -> List.rev acc
    | Token.Ident _ ->
        langs (parse_extern_lang_body ~parse_type ~parse_type_no_error st :: acc)
    | _ ->
        P.error st (P.peek st).span "unexpected token in op body";
        ignore (P.advance st);
        langs acc
  in
  let ls = langs [] in
  let close = P.expect st Token.RBrace "'}' to close the op body" in
  {
    Ast.ed_name = name;
    ed_name_span = name_span;
    ed_traits = traits;
    ed_params = params;
    ed_return = ret;
    ed_langs = ls;
    ed_span =
      Span.merge kw.span (match close with Some t -> t.span | None -> kw.span);
  }

(* Traits written above an item of an ext body: they must be followed by
   "op" (the only declaration here that takes traits). *)
let parse_item_traits st : Ast.trait list =
  match (P.peek st).kind with
  | Token.At ->
      let first = (P.peek st).span in
      let traits = Parser_traits.parse_leading_traits st in
      if (P.peek st).kind = Token.KwOp then traits
      else (
        P.error st first
          (Printf.sprintf "expected an op after its traits, found %s"
             (Token.describe (P.peek st).kind));
        [])
  | _ -> []

(* struct ::= "struct" name "{" (field | lang_block | traits? op)* "}"
   -- one grammar for both shapes the block declares, told apart by
   content: fields make it a foreign form (data the target reads), their
   absence makes it an opaque handle (a thing the target calls). A struct
   with both is diagnosed: a foreign form has no methods and a handle has
   no fields. *)
type ext_struct =
  | Foreign_form of Ast.foreign_struct
  | Handle of Ast.opaque_type

let parse_ext_struct ~parse_type ~parse_type_no_error st : ext_struct =
  let kw = P.advance st in
  (* 'struct' *)
  let name, name_span = expect_ident st "a struct name" in
  check_not_error_name st "struct" name name_span;
  ignore (P.expect st Token.LBrace "'{' to open the struct body");
  let fields = ref [] in
  let langs = ref [] in
  let methods = ref [] in
  let rec go () =
    let traits = parse_item_traits st in
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof -> ()
    | Token.Comma ->
        ignore (P.advance st);
        go ()
    | Token.KwOp ->
        methods :=
          parse_extern ~parse_type ~parse_type_no_error st ~traits :: !methods;
        go ()
    | Token.Ident _ when (P.peek_ahead st 1).kind = Token.LBrace ->
        langs := parse_lang_block st :: !langs;
        go ()
    | Token.Ident _ ->
        fields := parse_foreign_field ~parse_type st :: !fields;
        go ()
    | _ ->
        P.error st (P.peek st).span
          "unexpected token in struct body: expected a field, a language \
           block, or an op";
        ignore (P.advance st);
        go ()
  in
  go ();
  let close = P.expect st Token.RBrace "'}' to close the struct body" in
  let span =
    Span.merge kw.span (match close with Some t -> t.span | None -> kw.span)
  in
  let fields = List.rev !fields and langs = List.rev !langs in
  let methods = List.rev !methods in
  match (fields, methods) with
  | _ :: _, _ :: _ ->
      P.error st name_span
        (Printf.sprintf
           "'%s' declares both fields and ops: a foreign form has no methods \
            and an opaque handle has no fields"
           name);
      Handle
        {
          Ast.opq_name = name;
          opq_name_span = name_span;
          opq_langs = langs;
          opq_methods = methods;
          opq_span = span;
        }
  | _ :: _, [] ->
      Foreign_form
        {
          Ast.fs_name = name;
          fs_name_span = name_span;
          fs_fields = fields;
          fs_langs = langs;
          fs_span = span;
        }
  | [], _ ->
      Handle
        {
          Ast.opq_name = name;
          opq_name_span = name_span;
          opq_langs = langs;
          opq_methods = methods;
          opq_span = span;
        }

(* ext_lib_body ::= (lang_block | struct | traits? op)* *)
let parse_ext_lib_body ~parse_type ~parse_type_no_error st : Ast.ext_lib_body =
  let langs = ref [] in
  let structs = ref [] in
  let types = ref [] in
  let externs = ref [] in
  let rec go () =
    let traits = parse_item_traits st in
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof -> ()
    | Token.Comma ->
        ignore (P.advance st);
        go ()
    | Token.KwStruct ->
        (match parse_ext_struct ~parse_type ~parse_type_no_error st with
        | Foreign_form s -> structs := s :: !structs
        | Handle t -> types := t :: !types);
        go ()
    | Token.KwOp ->
        externs :=
          parse_extern ~parse_type ~parse_type_no_error st ~traits :: !externs;
        go ()
    | Token.Ident _ when (P.peek_ahead st 1).kind = Token.LBrace ->
        langs := parse_lang_path st :: !langs;
        go ()
    | _ ->
        P.error st (P.peek st).span
          "unexpected token in an ext block: expected a language block, \
           'struct', or 'op'";
        ignore (P.advance st);
        go ()
  in
  go ();
  {
    Ast.elib_langs = List.rev !langs;
    elib_structs = List.rev !structs;
    elib_types = List.rev !types;
    elib_externs = List.rev !externs;
  }

(* ext_lib ::= "ext" name "{" ext_lib_body "}"  -- "ext" and [name] have
   already been consumed by the caller, which used the lookahead after "ext"
   to decide this is the library form rather than the legacy
   hook/contract/constraint/impl form. *)
let parse_ext_lib ~parse_type ~parse_type_no_error st ~pub ~dtraits ~name
    ~name_span : Ast.decl =
  ignore (P.expect st Token.LBrace "'{' to open the ext body");
  let body = parse_ext_lib_body ~parse_type ~parse_type_no_error st in
  let close = P.expect st Token.RBrace "'}' to close the ext body" in
  let span =
    Span.merge name_span
      (match close with Some t -> t.span | None -> name_span)
  in
  {
    Ast.dname = name;
    dname_span = name_span;
    pub;
    dtraits;
    dkind = Ast.DExtLib { body; span };
  }
