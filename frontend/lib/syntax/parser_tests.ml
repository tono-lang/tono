(* The test-block grammar: [test "name" { ... }] with construction, stub, and
   call bindings plus [expect] assertions. The value forms replicate the shape
   of the calculus ctor/literal subset rather than reusing [Calc_parser]: the
   calculus runs on its own token stream, while a test block lives inside the
   main .tono stream, and its value language is a small closed subset (literals,
   ctor, list, map, binding reference) with binding scoping the calculus does
   not have. [stub], [expect], [any], [None], and [ok] are contextual: they are
   only special inside a test block, and only where an identifier could not
   otherwise appear with that meaning. *)

module P = Parser_state

let ident st what : string * Span.span =
  let t = P.peek st in
  match t.Token.kind with
  | Token.Ident n ->
      ignore (P.advance st);
      (n, t.span)
  | _ ->
      P.error st t.span
        (Printf.sprintf "expected %s, found %s" what (Token.describe t.kind));
      ("", t.span)

(* A dotted identifier path already begun with [head]: keep consuming
   '.' ident pairs. Returns the segments in order and the merged span. *)
let dotted_tail st (head : string) (head_span : Span.span) :
    string list * Span.span =
  let rec go acc last =
    match ((P.peek st).kind, (P.peek_ahead st 1).kind) with
    | Token.Dot, Token.Ident _ ->
        ignore (P.advance st);
        let s, sp = ident st "an identifier after '.'" in
        go (s :: acc) sp
    | _ -> (List.rev acc, last)
  in
  let segs, finish = go [ head ] head_span in
  (segs, Span.merge head_span finish)

(* A map key: an identifier or a string literal (header names carry dashes). *)
let map_key st : (string * Span.span) option =
  let t = P.peek st in
  match t.Token.kind with
  | Token.Ident n ->
      ignore (P.advance st);
      Some (n, t.span)
  | Token.Str s ->
      ignore (P.advance st);
      Some (s, t.span)
  | _ -> None

(* ── Values ────────────────────────────────────────────────────────────── *)

let rec parse_value st : Ast.test_value =
  let t = P.peek st in
  match t.Token.kind with
  | Token.Str s ->
      ignore (P.advance st);
      Ast.TvStr (s, t.span)
  | Token.Int n ->
      ignore (P.advance st);
      Ast.TvInt (n, t.span)
  | Token.Float f ->
      ignore (P.advance st);
      Ast.TvFloat (f, t.span)
  | Token.LBracket -> parse_value_list st t
  | Token.LBrace -> parse_value_map st t
  | Token.Ident ("true" | "false") ->
      ignore (P.advance st);
      Ast.TvBool (t.kind = Token.Ident "true", t.span)
  | Token.Ident ("any" | "None") ->
      ignore (P.advance st);
      P.error st t.span
        (Printf.sprintf
           "%s is an expect-pattern mark and cannot appear in a value"
           (Token.describe t.kind));
      Ast.TvError t.span
  | Token.Ident name -> (
      ignore (P.advance st);
      let segs, span = dotted_tail st name t.span in
      match (P.peek st).kind with
      | Token.LBrace ->
          let fields, finish = parse_ctor_fields st in
          Ast.TvCtor
            {
              tc_head = { vh_segs = segs; vh_span = span };
              tc_fields = fields;
              tc_span = Span.merge t.span finish;
            }
      | _ -> Ast.TvRef { base = name; path = List.tl segs; ref_span = span })
  | _ ->
      P.error st t.span
        (Printf.sprintf "expected a value, found %s" (Token.describe t.kind));
      ignore (P.advance st);
      Ast.TvError t.span

(* '[' value (',' value)* ']' -- the empty list parses (and is then rejected by
   the typechecker where a sequence needs at least one answer). *)
and parse_value_list st lb : Ast.test_value =
  ignore (P.advance st);
  let rec go acc =
    match (P.peek st).kind with
    | Token.RBracket | Token.Eof -> List.rev acc
    | Token.Comma ->
        ignore (P.advance st);
        go acc
    | _ -> go (parse_value st :: acc)
  in
  let items = go [] in
  let close = P.expect st Token.RBracket "']' to close the list" in
  let finish = match close with Some c -> c.span | None -> lb.Token.span in
  Ast.TvList (items, Span.merge lb.Token.span finish)

(* A headless brace in value position is a map literal (string-keyed). *)
and parse_value_map st lb : Ast.test_value =
  ignore (P.advance st);
  let rec go acc =
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof -> List.rev acc
    | Token.Comma ->
        ignore (P.advance st);
        go acc
    | _ -> (
        match map_key st with
        | Some key ->
            ignore (P.expect st Token.Colon "':' after the map key");
            go ((key, parse_value st) :: acc)
        | None ->
            P.error st (P.peek st).span
              "expected a map key (an identifier or a string)";
            ignore (P.advance st);
            go acc)
  in
  let entries = go [] in
  let close = P.expect st Token.RBrace "'}' to close the map" in
  let finish = match close with Some c -> c.span | None -> lb.Token.span in
  Ast.TvMap (entries, Span.merge lb.Token.span finish)

(* '{' (name ':' value)* '}' -- ctor fields; a '..' here belongs to patterns and
   is diagnosed in place. *)
and parse_ctor_fields st :
    (string * Span.span * Ast.test_value) list * Span.span =
  let lb = P.advance st in
  let rec go acc =
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof -> List.rev acc
    | Token.Comma ->
        ignore (P.advance st);
        go acc
    | Token.Dot when (P.peek_ahead st 1).kind = Token.Dot ->
        P.error st (P.peek st).span
          "'..' frees uncited fields in an expect pattern; a value constructor \
           simply omits them";
        ignore (P.advance st);
        ignore (P.advance st);
        go acc
    | Token.Ident _ ->
        let name, name_span = ident st "a field name" in
        ignore (P.expect st Token.Colon "':' after the field name");
        go ((name, name_span, parse_value st) :: acc)
    | _ ->
        P.error st (P.peek st).span
          (Printf.sprintf "unexpected %s in the constructor body"
             (Token.describe (P.peek st).kind));
        ignore (P.advance st);
        go acc
  in
  let fields = go [] in
  let close = P.expect st Token.RBrace "'}' to close the constructor" in
  let finish = match close with Some c -> c.span | None -> lb.Token.span in
  (fields, finish)

(* ── Patterns ──────────────────────────────────────────────────────────── *)

let rec parse_pattern st : Ast.test_pattern =
  let t = P.peek st in
  match t.Token.kind with
  | Token.Str _ | Token.Int _ | Token.Float _ -> Ast.TpLit (parse_value st)
  | Token.LBracket -> parse_pattern_list st t
  | Token.LBrace ->
      let entries, map_open, finish = parse_map_pattern_body st in
      Ast.TpMap { entries; map_open; map_span = Span.merge t.span finish }
  | Token.Ident ("true" | "false") -> Ast.TpLit (parse_value st)
  | Token.Ident "ok" when (P.peek_ahead st 1).kind <> Token.LBrace ->
      ignore (P.advance st);
      Ast.TpOk t.span
  | Token.Ident name -> (
      ignore (P.advance st);
      let segs, span = dotted_tail st name t.span in
      match (P.peek st).kind with
      | Token.LBrace ->
          let fields, open_, finish = parse_pattern_fields st in
          Ast.TpCtor
            {
              tp_head = { vh_segs = segs; vh_span = span };
              tp_fields = fields;
              tp_open = open_;
              tp_span = Span.merge t.span finish;
            }
      | _ ->
          Ast.TpLit
            (Ast.TvRef { base = name; path = List.tl segs; ref_span = span }))
  | _ ->
      P.error st t.span
        (Printf.sprintf "expected an expect pattern, found %s"
           (Token.describe t.kind));
      ignore (P.advance st);
      Ast.TpError t.span

and parse_pattern_list st lb : Ast.test_pattern =
  ignore (P.advance st);
  let rec go acc =
    match (P.peek st).kind with
    | Token.RBracket | Token.Eof -> List.rev acc
    | Token.Comma ->
        ignore (P.advance st);
        go acc
    | _ -> go (parse_pattern st :: acc)
  in
  let items = go [] in
  let close = P.expect st Token.RBracket "']' to close the pattern list" in
  let finish = match close with Some c -> c.span | None -> lb.Token.span in
  Ast.TpList (items, Span.merge lb.Token.span finish)

and parse_pattern_field st : Ast.test_pattern_field =
  let t = P.peek st in
  match t.Token.kind with
  (* [any { ... }] would be a ctor pattern of a shape named any; the bare word
     is the presence mark. Same for a bare [None]. *)
  | Token.Ident "any" when (P.peek_ahead st 1).kind <> Token.LBrace ->
      ignore (P.advance st);
      Ast.TpfAny t.span
  | Token.Ident "None" when (P.peek_ahead st 1).kind <> Token.LBrace ->
      ignore (P.advance st);
      Ast.TpfAbsent t.span
  | _ -> Ast.TpfPat (parse_pattern st)

(* The body of a ctor pattern: fields, with '..' anywhere marking it open. The
   opening '{' is at the cursor. Returns fields, the open flag, and the span of
   the closing brace. *)
and parse_pattern_fields st :
    (string * Span.span * Ast.test_pattern_field) list * bool * Span.span =
  let lb = P.advance st in
  let open_ = ref false in
  let rec go acc =
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof -> List.rev acc
    | Token.Comma ->
        ignore (P.advance st);
        go acc
    | Token.Dot when (P.peek_ahead st 1).kind = Token.Dot ->
        ignore (P.advance st);
        ignore (P.advance st);
        open_ := true;
        go acc
    | Token.Ident _ ->
        let name, name_span = ident st "a field name" in
        ignore (P.expect st Token.Colon "':' after the field name");
        go ((name, name_span, parse_pattern_field st) :: acc)
    | _ ->
        P.error st (P.peek st).span
          (Printf.sprintf "unexpected %s in the pattern body"
             (Token.describe (P.peek st).kind));
        ignore (P.advance st);
        go acc
  in
  let fields = go [] in
  let close = P.expect st Token.RBrace "'}' to close the pattern" in
  let finish = match close with Some c -> c.span | None -> lb.Token.span in
  (fields, !open_, finish)

(* A headless brace in pattern position: a map subset (header maps). Keys may be
   strings; '..' is tolerated (a map pattern is a subset by name either way). *)
and parse_map_pattern_body st :
    ((string * Span.span) * Ast.test_pattern_field) list * bool * Span.span =
  let lb = P.advance st in
  let open_ = ref false in
  let rec go acc =
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof -> List.rev acc
    | Token.Comma ->
        ignore (P.advance st);
        go acc
    | Token.Dot when (P.peek_ahead st 1).kind = Token.Dot ->
        ignore (P.advance st);
        ignore (P.advance st);
        open_ := true;
        go acc
    | _ -> (
        match map_key st with
        | Some key ->
            ignore (P.expect st Token.Colon "':' after the map key");
            go ((key, parse_pattern_field st) :: acc)
        | None ->
            P.error st (P.peek st).span
              "expected a map key (an identifier or a string)";
            ignore (P.advance st);
            go acc)
  in
  let entries = go [] in
  let close = P.expect st Token.RBrace "'}' to close the map pattern" in
  let finish = match close with Some c -> c.span | None -> lb.Token.span in
  (entries, !open_, finish)

(* ── Items ─────────────────────────────────────────────────────────────── *)

(* stub_target ::= ident ('.' ident){1,2} -- a dotted path of 2 or 3
   identifiers. Typecheck resolves the shape: "binding.op.dep", "ext_lib.fn",
   or "ext_lib.type.method". *)
let parse_stub_target st : Ast.stub_target =
  let first, fspan = ident st "the stub target" in
  let rec go acc last_span =
    if (P.peek st).kind = Token.Dot && List.length acc < 3 then (
      ignore (P.advance st);
      let seg, sspan = ident st "a segment of the stub target" in
      go ((seg, sspan) :: acc) sspan)
    else (List.rev acc, last_span)
  in
  let path, last_span = go [ (first, fspan) ] fspan in
  if List.length path < 2 then
    P.error st (P.peek st).span
      "expected '.' after the stub target's first segment";
  { Ast.st_path = path; st_span = Span.merge fspan last_span }

(* stub ::= "stub" stub_target ":" value -- the leading keyword and any
   [name ":"] binding are already consumed. *)
let parse_stub st ~bind ~start : Ast.test_item =
  let target = parse_stub_target st in
  ignore (P.expect st Token.Colon "':' before the stub value");
  let value = parse_value st in
  let finish =
    match value with
    | Ast.TvStr (_, s)
    | Ast.TvInt (_, s)
    | Ast.TvFloat (_, s)
    | Ast.TvBool (_, s)
    | Ast.TvCtor { tc_span = s; _ }
    | Ast.TvList (_, s)
    | Ast.TvMap (_, s)
    | Ast.TvRef { ref_span = s; _ }
    | Ast.TvError s ->
        s
  in
  Ast.TiStub { bind; target; value; item_span = Span.merge start finish }

(* expect ::= "expect" subject ('.' "requests")? ":" pattern *)
let parse_expect st ~start : Ast.test_item =
  let subject, subject_span = ident st "the binding an expect asserts" in
  let requests =
    match (P.peek st).kind with
    | Token.Dot ->
        ignore (P.advance st);
        let field, fspan = ident st "'requests' after '.'" in
        if String.equal field "requests" then true
        else (
          P.error st fspan
            "the only projection an expect reads is '.requests' on a stub \
             binding";
          true)
    | _ -> false
  in
  ignore (P.expect st Token.Colon "':' before the expect pattern");
  let pattern = parse_pattern st in
  Ast.TiExpect { subject; subject_span; requests; pattern; item_span = start }

(* The right-hand side of a binding [name: ...]: a stub, a construction
   ([entry { ... }]), or a call ([recv.op(input?)]). *)
let parse_binding_rhs st ~bind ~bind_span ~start : Ast.test_item option =
  match ((P.peek st).kind, (P.peek_ahead st 1).kind) with
  | Token.Ident "stub", Token.Ident _ ->
      ignore (P.advance st);
      Some (parse_stub st ~bind:(Some (bind, bind_span)) ~start)
  | Token.Ident _, _ -> (
      let head, head_span = ident st "an entry or binding name" in
      match (P.peek st).kind with
      | Token.LBrace ->
          let fields, finish = parse_ctor_fields st in
          Some
            (Ast.TiConstruct
               {
                 bind;
                 bind_span;
                 entry = head;
                 entry_span = head_span;
                 fields;
                 item_span = Span.merge start finish;
               })
      | Token.Dot ->
          ignore (P.advance st);
          let op, op_span = ident st "an operation name after '.'" in
          ignore (P.expect st Token.LParen "'(' to open the call input");
          let input =
            match (P.peek st).kind with
            | Token.RParen -> None
            | _ -> Some (parse_value st)
          in
          let close = P.expect st Token.RParen "')' to close the call input" in
          let finish =
            match close with Some c -> c.Token.span | None -> op_span
          in
          Some
            (Ast.TiCall
               {
                 bind;
                 bind_span;
                 recv = head;
                 recv_span = head_span;
                 op;
                 op_span;
                 input;
                 item_span = Span.merge start finish;
               })
      | _ ->
          P.error st (P.peek st).span
            "expected '{' (a construction) or '.' (a call) after the name";
          None)
  | _ ->
      P.error st (P.peek st).span
        (Printf.sprintf
           "expected a construction, a call, or a stub after ':', found %s"
           (Token.describe (P.peek st).kind));
      None

let parse_items st : Ast.test_item list =
  let rec go acc =
    let t = P.peek st in
    match (t.Token.kind, (P.peek_ahead st 1).kind) with
    | (Token.RBrace | Token.Eof), _ -> List.rev acc
    | Token.Comma, _ ->
        ignore (P.advance st);
        go acc
    | Token.Ident "expect", k when k <> Token.Colon ->
        ignore (P.advance st);
        let item = parse_expect st ~start:t.span in
        go (item :: acc)
    | Token.Ident "stub", Token.Ident _ ->
        ignore (P.advance st);
        let item = parse_stub st ~bind:None ~start:t.span in
        go (item :: acc)
    | Token.Ident name, Token.Colon ->
        ignore (P.advance st);
        ignore (P.advance st);
        let item =
          parse_binding_rhs st ~bind:name ~bind_span:t.span ~start:t.span
        in
        go (match item with Some i -> i :: acc | None -> acc)
    | _ ->
        P.error st t.span
          (Printf.sprintf
             "expected a binding, 'stub', or 'expect' in the test body, found \
              %s"
             (Token.describe t.kind));
        ignore (P.advance st);
        go acc
  in
  go []

(* test ::= "test" string "{" item* "}" -- a test takes no visibility and no
   traits; a [pub] or leading trait is diagnosed by the caller. *)
let parse_test st : Ast.decl =
  let kw = P.advance st in
  (* 'test' *)
  let name, name_span =
    match (P.peek st).kind with
    | Token.Str s ->
        let t = P.advance st in
        (s, t.span)
    | _ ->
        P.error st (P.peek st).span
          "expected the test name as a string after 'test'";
        ("", kw.Token.span)
  in
  ignore (P.expect st Token.LBrace "'{' to open the test body");
  let titems = parse_items st in
  ignore (P.expect st Token.RBrace "'}' to close the test body");
  {
    Ast.dname = name;
    dname_span = name_span;
    pub = false;
    dtraits = [];
    dkind = Ast.DTest { titems };
  }
