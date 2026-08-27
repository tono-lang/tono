(* The two sum declarations, union and enum. They share the body shape (a
   comma-tolerant list of named items, each with the traits written above it
   and inline after it) and nothing else in the grammar depends on them. *)

module P = Parser_state

let parse_type = Parser_type.parse_type
let parse_inline_traits = Parser_traits.parse_inline_traits
let parse_item_traits = Parser_traits.parse_item_traits

(* ── Unions ────────────────────────────────────────────────────────────── *)

(* variant ::= trait* name ( "(" type ")" )? trait*  — the name token is
   already consumed and passed in, so the only caller that reaches here had
   an identifier in hand; [leading] are the traits written above it. *)
let parse_variant st ~leading ~name ~name_span : Ast.union_variant =
  let payload =
    match (P.peek st).kind with
    | Token.LParen ->
        ignore (P.advance st);
        let t = parse_type st in
        ignore (P.expect st Token.RParen "')' to close the variant payload");
        Some t
    | _ -> None
  in
  let traits = parse_inline_traits st in
  {
    Ast.vname = name;
    vname_span = name_span;
    vpayload = payload;
    vtraits = leading @ traits;
  }

let parse_variants st : Ast.union_variant list =
  let starts_item = function Token.Ident _ -> true | _ -> false in
  let rec go acc =
    let leading = parse_item_traits st ~what:"variant" ~starts_item in
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof -> List.rev acc
    | Token.Ident name ->
        let nt = P.advance st in
        go (parse_variant st ~leading ~name ~name_span:nt.span :: acc)
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
  Parser_extern.check_not_error_name st "union" name nt.span;
  let params = Parser_type.parse_generics st in
  (* traits after the name (e.g. @discriminator) join the shape-level traits *)
  let dtraits = dtraits @ parse_inline_traits st in
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

(* ── Enums ─────────────────────────────────────────────────────────────── *)

(* case ::= trait* name ("=" int)? trait*  — the name token is already
   consumed and passed in, so the only caller that reaches here had an
   identifier in hand; [leading] are the traits written above it. *)
let parse_enum_case st ~leading ~name ~name_span : Ast.enum_case =
  (* A case named 'null' would be unreachable as a match pattern: the match
     parser treats a bare 'null' token as the optional-subject absence
     pattern regardless of the enum it matches, so a case with this name
     could never be selected. Reject it at the declaration site instead of
     silently reinterpreting the pattern later. *)
  if String.equal name "null" then
    P.error st name_span
      "'null' is reserved for the optional-subject absence pattern in a match; \
       an enum case cannot be named 'null'";
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
  let traits = parse_inline_traits st in
  { Ast.cname = name; cname_span = name_span; cint; ctraits = leading @ traits }

let parse_enum_cases st : Ast.enum_case list =
  let starts_item = function Token.Ident _ -> true | _ -> false in
  let rec go acc =
    let leading = parse_item_traits st ~what:"case" ~starts_item in
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof -> List.rev acc
    | Token.Ident name ->
        let nt = P.advance st in
        go (parse_enum_case st ~leading ~name ~name_span:nt.span :: acc)
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
  Parser_extern.check_not_error_name st "enum" name nt.span;
  (* an enum carries no positional trait after its name (every enum is open;
     shape-level traits like @doc are leading, handled by parse_decl) *)
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
