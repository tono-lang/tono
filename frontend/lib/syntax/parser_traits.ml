(* Trait and field-reference parsing, shared by every declaration form: the
   "@" trait grammar (with "::" catalog names and reference/template argument
   values) and the ".a.b" field-reference paths the entry model introduced.
   Same discipline as [Parser]: single-token lookahead, never raises. *)

module P = Parser_state

(* ref ::= "." name ("." name)*  — a field reference, possibly a path into a
   structured field. The caller has already seen the leading dot. *)
let parse_ref_path st : Ast.ref_path =
  let d0 = P.advance st in
  (* '.' *)
  let seg () =
    match (P.peek st).kind with
    | Token.Ident n ->
        let t = P.advance st in
        (n, t.span)
    | _ ->
        P.error st (P.peek st).span "expected a field name after '.'";
        ("", (P.peek st).span)
  in
  let first, fspan = seg () in
  let rec more acc last =
    match (P.peek st).kind with
    | Token.Dot ->
        ignore (P.advance st);
        let s, sp = seg () in
        more (s :: acc) sp
    | _ -> (List.rev acc, last)
  in
  let segs, last = more [ first ] fspan in
  { Ast.segs; ref_span = Span.merge d0.span last }

(* A trait-argument value (the part after "key:"): a scalar, or a
   "[" v ("," v)* "]" list literal, e.g. @http(code: [200, 207]), or a
   "name { field: value, ... }" ctor, e.g. @body(note_body { title: .x }). *)
let rec parse_trait_value st : Ast.trait_arg =
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
      let nt = P.advance st in
      match (P.peek st).kind with
      | Token.LBrace -> parse_ctor_arg st n nt.span
      | _ -> Ast.AName n)
  | Token.Dot -> Ast.ARef (parse_ref_path st)
  | Token.LBracket -> parse_trait_list_value st
  | _ ->
      P.error st t.span "expected a value after ':'";
      Ast.AName ""

(* ctor ::= name "{" ( field ":" value ( "," field ":" value )* )? "}" — the
   caller has already consumed [name] at [name_span]. *)
and parse_ctor_arg st (name : string) (name_span : Span.span) : Ast.trait_arg =
  let lb = P.advance st in
  (* '{' *)
  let field () =
    match (P.peek st).kind with
    | Token.Ident fname ->
        let ft = P.advance st in
        ignore (P.expect st Token.Colon "':' after a ctor field name");
        (fname, ft.span, parse_trait_value st)
    | _ ->
        P.error st (P.peek st).span "expected a field name";
        ("", (P.peek st).span, Ast.AName "")
  in
  let fields =
    match (P.peek st).kind with
    | Token.RBrace -> []
    | _ ->
        let first = field () in
        let rec more acc =
          match (P.peek st).kind with
          | Token.Comma ->
              ignore (P.advance st);
              if (P.peek st).kind = Token.RBrace then List.rev acc
              else more (field () :: acc)
          | _ -> List.rev acc
        in
        more [ first ]
  in
  let rb_span =
    match P.expect st Token.RBrace "'}' to close a ctor" with
    | Some t -> t.span
    | None -> (P.peek st).span
  in
  Ast.ACtor
    {
      ctor_name = name;
      ctor_name_span = name_span;
      ctor_fields = fields;
      ctor_span = Span.merge lb.span rb_span;
    }

and parse_trait_list_value st : Ast.trait_arg =
  ignore (P.advance st);
  (* '[' *)
  match (P.peek st).kind with
  | Token.RBracket ->
      ignore (P.advance st);
      Ast.AList []
  | _ ->
      let first = parse_trait_value st in
      let rec more acc =
        match (P.peek st).kind with
        | Token.Comma ->
            ignore (P.advance st);
            more (parse_trait_value st :: acc)
        | _ -> List.rev acc
      in
      let values = more [ first ] in
      ignore (P.expect st Token.RBracket "']' to close a list value");
      Ast.AList values

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
  | Token.Dot -> Ast.ARef (parse_ref_path st)
  | Token.Ident n -> (
      let nt = P.advance st in
      match (P.peek st).kind with
      | Token.Colon ->
          ignore (P.advance st);
          Ast.AKv (n, parse_trait_value st)
      | Token.LBrace -> parse_ctor_arg st n nt.span
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

(* trait ::= "@" name ("::" name)* ( "(" arg ("," arg)* ")" )?  — the "::"
   segments name a builtin catalog entry (e.g. @str::trim); the stored trait
   name keeps the separator, so "str::trim" is one trait id. *)
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
  let rec extend name nspan =
    if (P.peek st).kind = Token.ColonColon then (
      ignore (P.advance st);
      match (P.peek st).kind with
      | Token.Ident n | Token.Prim n ->
          let t = P.advance st in
          extend (name ^ "::" ^ n) t.span
      | _ ->
          P.error st (P.peek st).span "expected a catalog entry name after '::'";
          (name, nspan))
    else (name, nspan)
  in
  let name, nspan = extend name nspan in
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
