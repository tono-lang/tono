(* The new "ext <name> { ... }" FFI library-block grammar: per-language
   module paths, foreign struct/opaque-handle declarations, and [extern]
   declarations with a per-language call/yields/returns/errors binding.
   Distinct from the legacy [ext hook|contract|constraint|impl] grammar in
   [Parser_ext], which this module does not touch. [parse_type] is threaded
   in (as in [Parser_ext]) to avoid a dependency cycle with [Parser]. *)

module P = Parser_state

(* [error] is reserved as a yields-position sentinel only; used as an
   ordinary type elsewhere (an extern's return type, a [returns:] type) is a
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
   an extern's return type or a [returns:] type (both go through
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

(* lang_path ::= lang ":" string *)
let parse_lang_path st : Ast.lang_path =
  let t = P.peek st in
  let lang =
    match t.kind with
    | Token.Ident n ->
        ignore (P.advance st);
        n
    | _ ->
        P.error st t.span "expected a language identifier";
        ""
  in
  ignore (P.expect st Token.Colon "':' after a language identifier");
  let path =
    match (P.peek st).kind with
    | Token.Str s ->
        ignore (P.advance st);
        s
    | _ ->
        P.error st (P.peek st).span "expected a module path string";
        ""
  in
  { Ast.lp_lang = lang; lp_lang_span = t.span; lp_path = path }

(* foreign_field ::= name ":" type *)
let parse_foreign_field ~parse_type st : Ast.foreign_field =
  let nt = P.peek st in
  let name =
    match nt.kind with
    | Token.Ident n ->
        ignore (P.advance st);
        n
    | _ ->
        P.error st nt.span "expected a foreign field name";
        ""
  in
  ignore (P.expect st Token.Colon "':' after a foreign field name");
  { Ast.ff_name = name; ff_name_span = nt.span; ff_type = parse_type st }

(* struct ::= "struct" name "{" (field ",")* "}"  -- reuses the real
   [struct] keyword; disambiguated purely by appearing inside an ext-lib
   body, never producing a top-level [DStruct]. *)
let parse_foreign_struct ~parse_type st : Ast.foreign_struct =
  let kw = P.advance st in
  (* 'struct' *)
  let nt = P.peek st in
  let name =
    match nt.kind with
    | Token.Ident n ->
        ignore (P.advance st);
        n
    | _ ->
        P.error st nt.span "expected a struct name";
        ""
  in
  check_not_error_name st "foreign struct" name nt.span;
  ignore (P.expect st Token.LBrace "'{' to open the struct body");
  let rec fields acc =
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof -> List.rev acc
    | Token.Comma ->
        ignore (P.advance st);
        fields acc
    | Token.Ident _ -> fields (parse_foreign_field ~parse_type st :: acc)
    | _ ->
        P.error st (P.peek st).span "unexpected token in struct body";
        ignore (P.advance st);
        fields acc
  in
  let flds = fields [] in
  let close = P.expect st Token.RBrace "'}' to close the struct body" in
  {
    Ast.fs_name = name;
    fs_name_span = nt.span;
    fs_fields = flds;
    fs_span =
      Span.merge kw.span (match close with Some t -> t.span | None -> kw.span);
  }

(* yields ::= "yields" ":" "(" (name ":" (type | "error"), ",")+ ")" *)
let parse_yields_ty ~parse_type st : Ast.yields_ty =
  match (P.peek st).kind with
  | Token.Ident "error" ->
      let t = P.advance st in
      Ast.YError t.span
  | _ -> Ast.YType (parse_type st)

let parse_yields ~parse_type st : Ast.yields_pos list =
  ignore (P.advance st);
  (* 'yields' *)
  ignore (P.expect st Token.Colon "':' after 'yields'");
  ignore (P.expect st Token.LParen "'(' to open the yields list");
  let one () =
    let nt = P.peek st in
    let name =
      match nt.kind with
      | Token.Ident n ->
          ignore (P.advance st);
          n
      | _ ->
          P.error st nt.span "expected a yields name";
          ""
    in
    ignore (P.expect st Token.Colon "':' after a yields name");
    {
      Ast.yp_name = name;
      yp_name_span = nt.span;
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
  let nt = P.peek st in
  let name =
    match nt.kind with
    | Token.Ident n ->
        ignore (P.advance st);
        n
    | _ ->
        P.error st nt.span "expected a returns field name";
        ""
  in
  ignore (P.expect st Token.Colon "':' after a returns field name");
  let value = parse_returns_value st in
  {
    Ast.rf_name = name;
    rf_name_span = nt.span;
    rf_value = value;
    rf_span = nt.span;
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

(* errors ::= "errors" ":" "{" (string "=>" name, ",")* "}" *)
let parse_errors st : Ast.error_map_entry list =
  ignore (P.advance st);
  (* 'errors' *)
  ignore (P.expect st Token.Colon "':' after 'errors'");
  ignore (P.expect st Token.LBrace "'{' to open the errors body");
  let one () =
    let st_t = P.peek st in
    let sentinel =
      match st_t.kind with
      | Token.Str s ->
          ignore (P.advance st);
          s
      | _ ->
          P.error st st_t.span "expected a sentinel string";
          ""
    in
    ignore (P.expect st Token.FatArrow "'=>' after an errors sentinel");
    let nt = P.peek st in
    let ty_name =
      match nt.kind with
      | Token.Ident n ->
          ignore (P.advance st);
          n
      | _ ->
          P.error st nt.span "expected an error type name";
          ""
    in
    {
      Ast.em_sentinel = sentinel;
      em_sentinel_span = st_t.span;
      em_type = ty_name;
      em_type_span = nt.span;
    }
  in
  let rec go acc =
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof -> List.rev acc
    | Token.Comma ->
        ignore (P.advance st);
        go acc
    | Token.Str _ -> go (one () :: acc)
    | _ ->
        P.error st (P.peek st).span "unexpected token in errors body";
        ignore (P.advance st);
        go acc
  in
  let es = go [] in
  ignore (P.expect st Token.RBrace "'}' to close the errors body");
  es

(* call ::= "call" ":" string "(" call_arg ("," call_arg)* ")" *)
let parse_call_line st : string * Span.span * Ast.call_arg list =
  ignore (P.advance st);
  (* 'call' *)
  ignore (P.expect st Token.Colon "':' after 'call'");
  let t = P.peek st in
  let symbol =
    match t.kind with
    | Token.Str s ->
        ignore (P.advance st);
        s
    | _ ->
        P.error st t.span "expected a foreign symbol string after 'call:'";
        ""
  in
  let args = Parser_traits.parse_call_args st in
  (symbol, t.span, args)

(* lang_block ::= lang "{" ("call:" | "yields:" | "returns:" | "errors:")* "}"
   -- "call:" is required; its absence is diagnosed but the rest still parses. *)
let parse_extern_lang_body ~parse_type ~parse_type_no_error st :
    Ast.extern_lang_body =
  let langt = P.peek st in
  let lang =
    match langt.kind with
    | Token.Ident n ->
        ignore (P.advance st);
        n
    | _ ->
        P.error st langt.span "expected a language identifier";
        ""
  in
  ignore (P.expect st Token.LBrace "'{' to open the language block");
  let call = ref None in
  let yields = ref None in
  let returns = ref None in
  let errors = ref [] in
  let sync = ref false in
  let infallible = ref false in
  let ctx = ref false in
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
    | Token.Ident "errors" ->
        errors := parse_errors st;
        go ()
    | Token.Ident "sync" ->
        ignore (P.advance st);
        sync := true;
        go ()
    | Token.Ident "infallible" ->
        ignore (P.advance st);
        infallible := true;
        go ()
    | Token.Ident "ctx" ->
        ignore (P.advance st);
        ctx := true;
        go ()
    | _ ->
        P.error st (P.peek st).span
          (Printf.sprintf "unexpected token in a language block: expected %s"
             (Ext_lib_vocab.quoted Ext_lib_vocab.lang_body_words));
        ignore (P.advance st);
        go ()
  in
  go ();
  let close = P.expect st Token.RBrace "'}' to close the language block" in
  let symbol, symbol_span, args =
    match !call with
    | Some c -> c
    | None ->
        P.error st langt.span "a language block requires a 'call:' line";
        ("", langt.span, [])
  in
  {
    Ast.elb_lang = lang;
    elb_lang_span = langt.span;
    elb_call_symbol = symbol;
    elb_call_symbol_span = symbol_span;
    elb_call_args = args;
    elb_yields = !yields;
    elb_returns = !returns;
    elb_errors = !errors;
    elb_sync = !sync;
    elb_infallible = !infallible;
    elb_ctx = !ctx;
    elb_span =
      Span.merge langt.span
        (match close with Some t -> t.span | None -> langt.span);
  }

(* extern_param ::= name ":" type *)
let parse_extern_params ~parse_type st : Ast.extern_param list =
  ignore (P.expect st Token.LParen "'(' after the extern name");
  let one () =
    let nt = P.peek st in
    let name =
      match nt.kind with
      | Token.Ident n ->
          ignore (P.advance st);
          n
      | _ ->
          P.error st nt.span "expected a parameter name";
          ""
    in
    ignore (P.expect st Token.Colon "':' after a parameter name");
    { Ast.ep_name = name; ep_name_span = nt.span; ep_type = parse_type st }
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
      ignore (P.expect st Token.RParen "')' to close extern parameters");
      ps

(* extern ::= "extern" name "(" params ")" ":" type "{" lang_block+ "}" --
   used both as a free declaration and (unchanged) as a type's method. *)
let parse_extern ~parse_type ~parse_type_no_error st : Ast.extern_decl =
  let kw = P.advance st in
  (* 'extern' *)
  let nt = P.peek st in
  let name =
    match nt.kind with
    | Token.Ident n ->
        ignore (P.advance st);
        n
    | _ ->
        P.error st nt.span "expected an extern name";
        ""
  in
  let params = parse_extern_params ~parse_type st in
  ignore
    (P.expect st Token.Colon
       "':' after extern parameters (a return type is required)");
  let ret = parse_type_no_error st ~ctx:"an extern return type" in
  ignore (P.expect st Token.LBrace "'{' to open the extern body");
  let rec langs acc =
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof -> List.rev acc
    | Token.Ident _ ->
        langs (parse_extern_lang_body ~parse_type ~parse_type_no_error st :: acc)
    | _ ->
        P.error st (P.peek st).span "unexpected token in extern body";
        ignore (P.advance st);
        langs acc
  in
  let ls = langs [] in
  let close = P.expect st Token.RBrace "'}' to close the extern body" in
  {
    Ast.ed_name = name;
    ed_name_span = nt.span;
    ed_params = params;
    ed_return = ret;
    ed_langs = ls;
    ed_span =
      Span.merge kw.span (match close with Some t -> t.span | None -> kw.span);
  }

(* instance ::= "(" names "," type ")"
   names    ::= string | lang ":" string ("," lang ":" string)*
   -- declares which instantiation of a foreign generic type this opaque
   handle names: the foreign type's own name (a string, so the origin stays
   visible) and the tono argument it is monomorphized with. One bare string
   names the type for every language; the keyed form names it per language,
   in the same "lang: string" shape the ext header's module paths use, for
   the case where the targets spell the same logical type differently. *)
let parse_opaque_instance ~parse_type st : Ast.opaque_instance =
  let open_p = P.expect st Token.LParen "'(' to open the instantiation" in
  let nt = P.peek st in
  let parse_name_string () : string * Span.span =
    let t = P.peek st in
    match t.kind with
    | Token.Str s ->
        ignore (P.advance st);
        (s, t.span)
    | _ ->
        P.error st t.span "expected the foreign type's name as a string";
        ("", t.span)
  in
  let names =
    match nt.kind with
    | Token.Ident _ when (P.peek_ahead st 1).kind = Token.Colon ->
        (* Keyed entries run until the element after a comma is no longer a
           "lang:" head; that element is the type argument. *)
        let rec entries acc =
          let lt = P.peek st in
          match (lt.kind, (P.peek_ahead st 1).kind) with
          | Token.Ident lang, Token.Colon ->
              ignore (P.advance st);
              ignore (P.advance st);
              let name, name_span = parse_name_string () in
              let entry =
                {
                  Ast.one_lang = lang;
                  one_lang_span = lt.span;
                  one_name = name;
                  one_name_span = name_span;
                }
              in
              if (P.peek st).kind = Token.Comma then (
                ignore (P.advance st);
                entries (entry :: acc))
              else List.rev (entry :: acc)
          | _ -> List.rev acc
        in
        Ast.OnPerLang (entries [])
    | _ ->
        let name, name_span = parse_name_string () in
        ignore
          (P.expect st Token.Comma
             "',' between the foreign name and the argument");
        Ast.OnShared (name, name_span)
  in
  let arg_span_start = (P.peek st).span in
  let arg = parse_type st in
  let close = P.expect st Token.RParen "')' to close the instantiation" in
  (* An instantiation names exactly one foreign name and one type argument;
     a second argument (or any other stray token) is not a distinct error
     per token, so skip to the close paren or the type body instead of
     cascading into "expected '{' to open the type body" and then "a type
     body may only contain 'extern' methods" once per leftover token. *)
  (match close with
  | Some _ -> ()
  | None ->
      let rec skip () =
        match (P.peek st).kind with
        | Token.RParen -> ignore (P.advance st)
        | Token.LBrace | Token.RBrace | Token.Eof -> ()
        | _ ->
            ignore (P.advance st);
            skip ()
      in
      skip ());
  {
    Ast.oi_names = names;
    oi_arg = arg;
    oi_arg_span = arg_span_start;
    oi_span =
      Span.merge
        (match open_p with Some t -> t.span | None -> nt.span)
        (match close with Some t -> t.span | None -> arg_span_start);
  }

(* type ::= "type" name instance? "interface"? "{" extern+ "}"  -- an opaque
   foreign handle; "type" is a contextual keyword here (not a lexer keyword),
   like "extern"/"call"/"yields"/"returns"/"errors". The "interface" marker
   declares the foreign type is abstract (held by value in Go), which tono
   cannot infer from a foreign name; a concrete struct handle writes
   nothing. *)
let parse_opaque_type ~parse_type ~parse_type_no_error st : Ast.opaque_type =
  let kw = P.advance st in
  (* 'type' *)
  let nt = P.peek st in
  let name =
    match nt.kind with
    | Token.Ident n ->
        ignore (P.advance st);
        n
    | _ ->
        P.error st nt.span "expected a type name";
        ""
  in
  check_not_error_name st "opaque type" name nt.span;
  let instance =
    match (P.peek st).kind with
    | Token.LParen -> Some (parse_opaque_instance ~parse_type st)
    | _ -> None
  in
  let interface =
    match (P.peek st).kind with
    | Token.Ident w when List.mem w Ext_lib_vocab.type_markers ->
        ignore (P.advance st);
        true
    | _ -> false
  in
  ignore (P.expect st Token.LBrace "'{' to open the type body");
  let rec methods acc =
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof -> List.rev acc
    | Token.Ident "extern" ->
        methods (parse_extern ~parse_type ~parse_type_no_error st :: acc)
    | _ ->
        P.error st (P.peek st).span
          "a type body may only contain 'extern' methods";
        ignore (P.advance st);
        methods acc
  in
  let ms = methods [] in
  let close = P.expect st Token.RBrace "'}' to close the type body" in
  {
    Ast.opq_name = name;
    opq_name_span = nt.span;
    opq_instance = instance;
    opq_interface = interface;
    opq_methods = ms;
    opq_span =
      Span.merge kw.span (match close with Some t -> t.span | None -> kw.span);
  }

(* ext_lib_body ::= (lang_path | struct | type | extern)* *)
let parse_ext_lib_body ~parse_type ~parse_type_no_error st : Ast.ext_lib_body =
  let langs = ref [] in
  let structs = ref [] in
  let types = ref [] in
  let externs = ref [] in
  let rec go () =
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof -> ()
    | Token.Comma ->
        ignore (P.advance st);
        go ()
    | Token.KwStruct ->
        structs := parse_foreign_struct ~parse_type st :: !structs;
        go ()
    | Token.Ident "type" ->
        types := parse_opaque_type ~parse_type ~parse_type_no_error st :: !types;
        go ()
    | Token.Ident "extern" ->
        externs := parse_extern ~parse_type ~parse_type_no_error st :: !externs;
        go ()
    | Token.Ident _ ->
        (* The only other shape a bare identifier introduces here is a
           "lang: string" module path. *)
        langs := parse_lang_path st :: !langs;
        go ()
    | _ ->
        P.error st (P.peek st).span
          (Printf.sprintf
             "unexpected token in an ext block: expected a language path, \
              'struct', or %s"
             (Ext_lib_vocab.quoted Ext_lib_vocab.block_words));
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
   to decide this is the new library form rather than the legacy
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
