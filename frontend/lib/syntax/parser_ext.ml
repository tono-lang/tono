(* The legacy [ext hook|contract|constraint|impl] grammar, split out of
   [Parser] to keep that file under the line-count cap. Untouched by the new
   [ext <name> { ... }] library-block grammar in [Parser_extern]. *)

module P = Parser_state

(* The kind word after "ext": hook | contract | constraint | impl. An
   unrecognized word is diagnosed and defaults to a hook so the rest of the body
   still parses. *)
let parse_ext_kind st : Ast.ext_kind * Span.span =
  let t = P.peek st in
  match t.kind with
  | Token.Ident "hook" ->
      ignore (P.advance st);
      (Ast.EHook, t.span)
  | Token.Ident "contract" ->
      ignore (P.advance st);
      (Ast.EContract, t.span)
  | Token.Ident "constraint" ->
      ignore (P.advance st);
      (Ast.EConstraint, t.span)
  | Token.Ident "impl" ->
      ignore (P.advance st);
      (Ast.EImpl, t.span)
  | _ ->
      P.error st t.span
        "expected an extension kind: 'hook', 'contract', 'constraint', or \
         'impl'";
      (Ast.EHook, t.span)

(* signature ::= "(" type ")" "->" type *)
let parse_ext_sig ~parse_type st : Ast.ext_sig =
  ignore (P.advance st);
  (* '(' *)
  let input = parse_type st in
  ignore (P.expect st Token.RParen "')' to close the signature input");
  ignore (P.expect st Token.Arrow "'->' between the signature input and output");
  let output = parse_type st in
  { Ast.esig_in = input; esig_out = output }

(* ext_body ::= ( lang ":" string | "conformance" ":" string )*  — a language
   tag binds to a "file#symbol" reference; the reserved key "conformance" binds
   the conformance vector reference. *)
let parse_ext_body st : Ast.ext_binding list * string option =
  let rec go bindings conformance =
    let t = P.peek st in
    match t.kind with
    | Token.RBrace | Token.Eof -> (List.rev bindings, conformance)
    | Token.Comma ->
        ignore (P.advance st);
        go bindings conformance
    | Token.Ident key ->
        ignore (P.advance st);
        ignore (P.expect st Token.Colon "':' after an extension body key");
        let value =
          match (P.peek st).kind with
          | Token.Str s ->
              ignore (P.advance st);
              s
          | _ ->
              P.error st (P.peek st).span
                "expected a \"file#symbol\" string in the extension body";
              ""
        in
        if key = "conformance" then go bindings (Some value)
        else
          go
            ({ Ast.lang = key; lang_span = t.span; target = value } :: bindings)
            conformance
    | _ ->
        P.error st t.span
          (Printf.sprintf "unexpected %s in the extension body"
             (Token.describe t.kind));
        ignore (P.advance st);
        go bindings conformance
  in
  go [] None

(* An impl names the operation it implements. The bare operation name is the
   normal form; "entry.op" disambiguates when two entries in one module declare
   the same operation name. Only an impl reads the dotted form: for the other
   kinds the name is a slot or a contract name, which is a single segment. *)
let parse_ext_name st ekind : string * Span.span =
  let nt = P.peek st in
  let head =
    match nt.kind with
    | Token.Ident n ->
        ignore (P.advance st);
        n
    | _ ->
        P.error st nt.span "expected an extension name";
        ""
  in
  match (ekind, (P.peek st).kind) with
  | Ast.EImpl, Token.Dot -> (
      ignore (P.advance st);
      match (P.peek st).kind with
      | Token.Ident n ->
          ignore (P.advance st);
          (head ^ "." ^ n, nt.span)
      | _ ->
          P.error st (P.peek st).span
            "expected an operation name after '.' in \"entry.op\"";
          (head, nt.span))
  | _ -> (head, nt.span)

(* ext ::= "ext" ext_kind name "raw"? signature? "{" ext_body "}"  — "raw" is
   consumed for every kind so a misplaced one is a typecheck diagnostic pointing
   at the word rather than a confusing parse error. *)
let parse_ext ~parse_type st ~pub ~dtraits : Ast.decl =
  ignore (P.advance st);
  (* 'ext' *)
  let ekind, ekind_span = parse_ext_kind st in
  let name, name_span = parse_ext_name st ekind in
  let eraw =
    match (P.peek st).kind with
    | Token.Ident "raw" ->
        let t = P.peek st in
        ignore (P.advance st);
        Some t.span
    | _ -> None
  in
  let esig =
    match (P.peek st).kind with
    | Token.LParen -> Some (parse_ext_sig ~parse_type st)
    | _ -> None
  in
  ignore (P.expect st Token.LBrace "'{' to open the extension body");
  let ebindings, econformance = parse_ext_body st in
  ignore (P.expect st Token.RBrace "'}' to close the extension body");
  {
    Ast.dname = name;
    dname_span = name_span;
    pub;
    dtraits;
    dkind = Ast.DExt { ekind; ekind_span; esig; eraw; ebindings; econformance };
  }
