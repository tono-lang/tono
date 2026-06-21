(* Hand-written recursive-descent parser: one function per grammar nonterminal,
   single-token lookahead. It builds the surface AST and accumulates diagnostics;
   it never raises. *)

module P = Parser_state

(* type ::= base "?"? *)
let rec parse_type st : Ast.ty =
  let base = parse_base st in
  match (P.peek st).kind with
  | Token.Question ->
      let q = P.advance st in
      Ast.TNullable (base, Span.merge (Ast.ty_span base) q.span)
  | _ -> base

and parse_base st : Ast.ty =
  let t = P.peek st in
  match t.kind with
  | Token.Prim p ->
      ignore (P.advance st);
      Ast.TPrim (p, t.span)
  | Token.KwMap -> parse_map st t
  | Token.LBracket -> parse_list st t
  | Token.Ident name -> parse_named st t name
  | _ ->
      P.error st t.span
        (Printf.sprintf "expected a type, found %s" (Token.describe t.kind));
      Ast.TError t.span

(* []T : a leading '[' must be followed by ']'. *)
and parse_list st lb =
  ignore (P.advance st);
  (* '[' *)
  (match (P.peek st).kind with
  | Token.RBracket -> ignore (P.advance st)
  | _ -> P.error st (P.peek st).span "expected ']' to form a list type '[]T'");
  (* The element is a [base] (no trailing '?'): a '?' after '[]T' or 'map[K]V'
     binds to the whole preceding type, captured by the outer [parse_type]. *)
  let elem = parse_base st in
  Ast.TList (elem, Span.merge lb.span (Ast.ty_span elem))

(* map[K]V *)
and parse_map st kw =
  ignore (P.advance st);
  (* 'map' *)
  ignore (P.expect st Token.LBracket "'[' after 'map'");
  let k = parse_type st in
  ignore (P.expect st Token.RBracket "']' in map type");
  let v = parse_base st in
  Ast.TMap (k, v, Span.merge kw.span (Ast.ty_span v))

(* name, or name '[' type (',' type)* ']' (generic application) *)
and parse_named st t name =
  ignore (P.advance st);
  (* name *)
  match (P.peek st).kind with
  | Token.LBracket ->
      ignore (P.advance st);
      (* '[' *)
      let args = parse_type_list st in
      let close = P.expect st Token.RBracket "']' to close generic arguments" in
      let finish = match close with Some c -> c.span | None -> t.span in
      Ast.TName (name, args, Span.merge t.span finish)
  | _ -> Ast.TName (name, [], t.span)

and parse_type_list st =
  let first = parse_type st in
  let rec more acc =
    match (P.peek st).kind with
    | Token.Comma ->
        ignore (P.advance st);
        let n = parse_type st in
        more (n :: acc)
    | _ -> List.rev acc
  in
  more [ first ]

(* ── Traits ────────────────────────────────────────────────────────────── *)

(* A scalar trait-argument value (the part after "key:"). *)
let parse_trait_value st : Ast.trait_arg =
  let t = P.peek st in
  match t.kind with
  | Token.Str s ->
      ignore (P.advance st);
      Ast.AString s
  | Token.Int n ->
      ignore (P.advance st);
      Ast.AInt n
  | Token.Float f ->
      ignore (P.advance st);
      Ast.AFloat f
  | Token.Ident n ->
      ignore (P.advance st);
      Ast.AName n
  | _ ->
      P.error st t.span "expected a value after ':'";
      Ast.AName ""

let parse_trait_arg st : Ast.trait_arg =
  let t = P.peek st in
  match t.kind with
  | Token.Str s ->
      ignore (P.advance st);
      Ast.AString s
  | Token.Int n ->
      ignore (P.advance st);
      Ast.AInt n
  | Token.Float f ->
      ignore (P.advance st);
      Ast.AFloat f
  | Token.Ident n -> (
      ignore (P.advance st);
      match (P.peek st).kind with
      | Token.Colon ->
          ignore (P.advance st);
          Ast.AKv (n, parse_trait_value st)
      | _ -> Ast.AName n)
  | _ ->
      P.error st t.span "expected a trait argument";
      ignore (P.advance st);
      Ast.AName ""

let parse_trait_args st : Ast.trait_arg list =
  ignore (P.advance st);
  (* '(' *)
  match (P.peek st).kind with
  | Token.RParen ->
      ignore (P.advance st);
      []
  | _ ->
      let first = parse_trait_arg st in
      let rec more acc =
        match (P.peek st).kind with
        | Token.Comma ->
            ignore (P.advance st);
            more (parse_trait_arg st :: acc)
        | _ -> List.rev acc
      in
      let args = more [ first ] in
      ignore (P.expect st Token.RParen "')' to close trait arguments");
      args

(* trait ::= "@" name ( "(" arg ("," arg)* ")" )? *)
let parse_trait st : Ast.trait =
  let at = P.advance st in
  (* '@' *)
  let name, nspan =
    match (P.peek st).kind with
    | Token.Ident n | Token.Prim n ->
        let t = P.advance st in
        (n, t.span)
    | _ ->
        P.error st (P.peek st).span "expected a trait name after '@'";
        ("", (P.peek st).span)
  in
  let args =
    if (P.peek st).kind = Token.LParen then parse_trait_args st else []
  in
  { Ast.tname = name; targs = args; tspan = Span.merge at.span nspan }

let parse_trailing_traits st : Ast.trait list =
  let rec go acc =
    if (P.peek st).kind = Token.At then go (parse_trait st :: acc)
    else List.rev acc
  in
  go []

(* ── Members ───────────────────────────────────────────────────────────── *)

(* member ::= name ":" type trait* *)
let parse_member st : Ast.member =
  let nt = P.peek st in
  let name =
    match nt.kind with
    | Token.Ident n ->
        ignore (P.advance st);
        n
    | _ ->
        P.error st nt.span "expected a member name";
        ""
  in
  ignore (P.expect st Token.Colon "':' after member name");
  let ty = parse_type st in
  let traits = parse_trailing_traits st in
  { Ast.mname = name; mname_span = nt.span; mtype = ty; mtraits = traits }

let parse_members st ~what : Ast.member list =
  let rec go acc =
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof -> List.rev acc
    | Token.Ident _ -> go (parse_member st :: acc)
    | Token.Comma ->
        ignore (P.advance st);
        go acc
    | _ ->
        P.error st (P.peek st).span
          (Printf.sprintf "unexpected %s in %s body"
             (Token.describe (P.peek st).kind)
             what);
        ignore (P.advance st);
        go acc
  in
  go []

(* generics ::= "[" name ("," name)* "]" *)
let parse_generics st : string list =
  if (P.peek st).kind <> Token.LBracket then []
  else (
    ignore (P.advance st);
    let one () =
      match (P.peek st).kind with
      | Token.Ident n ->
          ignore (P.advance st);
          n
      | _ ->
          P.error st (P.peek st).span "expected a type parameter name";
          ""
    in
    let first = one () in
    let rec more acc =
      match (P.peek st).kind with
      | Token.Comma ->
          ignore (P.advance st);
          more (one () :: acc)
      | _ -> List.rev acc
    in
    let ps = more [ first ] in
    ignore (P.expect st Token.RBracket "']' to close type parameters");
    ps)

(* Shared header+body for the brace-with-members shapes (struct, union): a name,
   optional generics, and a braced member list. [what] names the keyword for the
   diagnostics. *)
let parse_shape_body st ~what =
  let nt = P.peek st in
  let name =
    match nt.kind with
    | Token.Ident n ->
        ignore (P.advance st);
        n
    | _ ->
        P.error st nt.span (Printf.sprintf "expected a %s name" what);
        ""
  in
  let params = parse_generics st in
  ignore
    (P.expect st Token.LBrace (Printf.sprintf "'{' to open the %s body" what));
  let members = parse_members st ~what in
  ignore
    (P.expect st Token.RBrace (Printf.sprintf "'}' to close the %s body" what));
  (name, nt.span, params, members)

(* struct ::= "struct" name generics? "{" member* "}" *)
let parse_struct st ~pub ~dtraits : Ast.decl =
  ignore (P.advance st);
  (* 'struct' *)
  let name, span, params, members = parse_shape_body st ~what:"struct" in
  {
    Ast.dname = name;
    dname_span = span;
    pub;
    dtraits;
    dkind = Ast.DStruct { params; members };
  }

(* variant ::= name ( "(" type ")" )? trait*  — the name token is already
   consumed and passed in, so the only caller that reaches here had an
   identifier in hand. *)
let parse_variant st ~name ~name_span : Ast.union_variant =
  let payload =
    match (P.peek st).kind with
    | Token.LParen ->
        ignore (P.advance st);
        let t = parse_type st in
        ignore (P.expect st Token.RParen "')' to close the variant payload");
        Some t
    | _ -> None
  in
  let traits = parse_trailing_traits st in
  {
    Ast.vname = name;
    vname_span = name_span;
    vpayload = payload;
    vtraits = traits;
  }

let parse_variants st : Ast.union_variant list =
  let rec go acc =
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof -> List.rev acc
    | Token.Ident name ->
        let nt = P.advance st in
        go (parse_variant st ~name ~name_span:nt.span :: acc)
    | Token.Comma ->
        ignore (P.advance st);
        go acc
    | _ ->
        P.error st (P.peek st).span
          (Printf.sprintf "unexpected %s in union body"
             (Token.describe (P.peek st).kind));
        ignore (P.advance st);
        go acc
  in
  go []

(* union ::= "union" name generics? "{" variant* "}" *)
let parse_union st ~pub ~dtraits : Ast.decl =
  ignore (P.advance st);
  (* 'union' *)
  let nt = P.peek st in
  let name =
    match nt.kind with
    | Token.Ident n ->
        ignore (P.advance st);
        n
    | _ ->
        P.error st nt.span "expected a union name";
        ""
  in
  let params = parse_generics st in
  (* traits after the name (e.g. @discriminator) join the shape-level traits *)
  let dtraits = dtraits @ parse_trailing_traits st in
  ignore (P.expect st Token.LBrace "'{' to open the union body");
  let variants = parse_variants st in
  ignore (P.expect st Token.RBrace "'}' to close the union body");
  {
    Ast.dname = name;
    dname_span = nt.span;
    pub;
    dtraits;
    dkind = Ast.DUnion { params; variants };
  }

(* case ::= name ("=" int)? trait*  — the name token is already consumed and
   passed in, so the only caller that reaches here had an identifier in hand. *)
let parse_enum_case st ~name ~name_span : Ast.enum_case =
  (* A payload here means the author wanted a union; diagnose and skip it. *)
  (match (P.peek st).kind with
  | Token.LParen ->
      P.error st (P.peek st).span
        "enum cases carry no payload; use a 'union' for variants with data";
      ignore (P.advance st);
      ignore (parse_type st);
      ignore (P.expect st Token.RParen "')' to close the payload")
  | _ -> ());
  let cint =
    match (P.peek st).kind with
    | Token.Eq -> (
        ignore (P.advance st);
        match (P.peek st).kind with
        | Token.Int n ->
            ignore (P.advance st);
            Some n
        | _ ->
            P.error st (P.peek st).span "expected an integer after '='";
            None)
    | _ -> None
  in
  let traits = parse_trailing_traits st in
  { Ast.cname = name; cname_span = name_span; cint; ctraits = traits }

let parse_enum_cases st : Ast.enum_case list =
  let rec go acc =
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof -> List.rev acc
    | Token.Ident name ->
        let nt = P.advance st in
        go (parse_enum_case st ~name ~name_span:nt.span :: acc)
    | Token.Comma ->
        ignore (P.advance st);
        go acc
    | _ ->
        P.error st (P.peek st).span
          (Printf.sprintf "unexpected %s in enum body"
             (Token.describe (P.peek st).kind));
        ignore (P.advance st);
        go acc
  in
  go []

(* enum ::= "enum" name "{" case* "}" *)
let parse_enum st ~pub ~dtraits : Ast.decl =
  ignore (P.advance st);
  (* 'enum' *)
  let nt = P.peek st in
  let name =
    match nt.kind with
    | Token.Ident n ->
        ignore (P.advance st);
        n
    | _ ->
        P.error st nt.span "expected an enum name";
        ""
  in
  (* traits after the name (e.g. @open) join the shape-level traits *)
  let dtraits = dtraits @ parse_trailing_traits st in
  ignore (P.expect st Token.LBrace "'{' to open the enum body");
  let cases = parse_enum_cases st in
  ignore (P.expect st Token.RBrace "'}' to close the enum body");
  {
    Ast.dname = name;
    dname_span = nt.span;
    pub;
    dtraits;
    dkind = Ast.DEnum { cases };
  }

(* op ::= "op" name "(" type? ")" ( ":" type )? op_trait*  — the output type is
   optional, and errors are carried by a trailing "@errors(...)" trait. *)
let parse_op st ~pub ~dtraits : Ast.decl =
  ignore (P.advance st);
  (* 'op' *)
  let nt = P.peek st in
  let name =
    match nt.kind with
    | Token.Ident n ->
        ignore (P.advance st);
        n
    | _ ->
        P.error st nt.span "expected an operation name";
        ""
  in
  ignore (P.expect st Token.LParen "'(' after the operation name");
  let input =
    match (P.peek st).kind with
    | Token.RParen -> None
    | _ -> Some (parse_type st)
  in
  ignore (P.expect st Token.RParen "')' to close the operation input");
  let output =
    match (P.peek st).kind with
    | Token.Colon ->
        ignore (P.advance st);
        Some (parse_type st)
    | _ -> None
  in
  (* Trailing op traits (@http, @errors, @async, ...) join the shape traits;
     lowering lifts @errors into Operation.errors and bags the rest. *)
  let dtraits = dtraits @ parse_trailing_traits st in
  {
    Ast.dname = name;
    dname_span = nt.span;
    pub;
    dtraits;
    dkind = Ast.DOp { input; output };
  }

(* ── Declarations and files ────────────────────────────────────────────── *)

(* decl ::= trait* "pub"? (struct | union | enum | op). Returns [None] when the
   keyword is missing so the file loop can resynchronize. *)
let parse_decl st : Ast.decl option =
  let dtraits = parse_trailing_traits st in
  let pub =
    match (P.peek st).kind with
    | Token.KwPub ->
        ignore (P.advance st);
        true
    | _ -> false
  in
  match (P.peek st).kind with
  | Token.KwStruct -> Some (parse_struct st ~pub ~dtraits)
  | Token.KwUnion -> Some (parse_union st ~pub ~dtraits)
  | Token.KwEnum -> Some (parse_enum st ~pub ~dtraits)
  | Token.KwOp -> Some (parse_op st ~pub ~dtraits)
  | _ ->
      P.error st (P.peek st).span
        (Printf.sprintf
           "expected a declaration (struct, enum, union, or op), found %s"
           (Token.describe (P.peek st).kind));
      None

(* A declaration can start with a trait, [pub], or one of the shape keywords;
   resynchronization skips to the next such token. *)
let is_decl_start = function
  | Token.At | Token.KwPub | Token.KwStruct | Token.KwUnion | Token.KwEnum
  | Token.KwOp ->
      true
  | _ -> false

let parse_file st : Ast.file =
  let rec go acc =
    if P.at_eof st then List.rev acc
    else
      match parse_decl st with
      | Some d -> go (d :: acc)
      | None ->
          (* parse_decl already diagnosed; ensure progress, then skip to the
             next declaration boundary. *)
          if not (P.at_eof st) then ignore (P.advance st);
          while (not (P.at_eof st)) && not (is_decl_start (P.peek st).kind) do
            ignore (P.advance st)
          done;
          go acc
  in
  go []

let parse (src : string) : Ast.file * Diagnostic.t list =
  let toks, lex_diags = Lexer.tokenize src in
  let st = P.create toks in
  let file = parse_file st in
  (file, Diagnostic.sort (lex_diags @ P.diagnostics st))
