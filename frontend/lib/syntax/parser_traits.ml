(* Trait and field-reference parsing, shared by every declaration form: the
   "@" trait grammar (with "::" catalog names and reference/template argument
   values), the ".a.b" field-reference paths the entry model introduced, and
   the "ns.fn(args)" call-expression grammar the FFI ext/extern block adds.
   Same discipline as [Parser]: single-token lookahead, never raises. *)

module P = Parser_state

(* ref ::= "." name ("." name)*  — a field reference, possibly a path into a
   structured field. The caller has already seen the leading dot. *)
let rec parse_ref_path st : Ast.ref_path =
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
  { Ast.segs; index = None; ref_span = Span.merge d0.span last }

(* Same grammar as [parse_ref_path], plus an optional trailing "[" ref "]"
   single-level map index, e.g. ".cfg.by_segment[.seg]". Kept separate from
   [parse_ref_path] rather than folded into it: a map index is only
   meaningful where the whole path is resolved as a unit against current
   scope (today, only a match subject), not at every ref-path use site
   (trait args, call args, ...). *)
and parse_indexed_ref_path st : Ast.ref_path =
  let base = parse_ref_path st in
  match (P.peek st).kind with
  | Token.LBracket ->
      ignore (P.advance st);
      let index = parse_ref_path st in
      let close = P.expect st Token.RBracket "']' to close a map index" in
      let finish =
        match close with Some c -> c.span | None -> index.Ast.ref_span
      in
      {
        base with
        Ast.index = Some index;
        ref_span = Span.merge base.Ast.ref_span finish;
      }
  | _ -> base

(* A trait-argument value (the part after "key:"): a scalar, a
   "[" v ("," v)* "]" list literal, e.g. @http(code: [200, 207]), a
   "name { field: value, ... }" ctor, e.g. @body(note_body { title: .x }), or
   a "ns.fn(args)" call expression, e.g. @header("K", companyauth.sign(.request)). *)
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
      | Token.Dot -> Ast.ACall (parse_call_expr st ~ns:n ~ns_span:nt.span)
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

(* call_arg ::= ".path" | name | name "{" field ":" value, ... "}" | string
   "(" call_arg, ... ")" | "[" call_arg, ... "]" — the same shapes [call:]
   accepts: a field-reference path, the caller's own bare parameter, a
   struct-literal mapper (reusing [parse_ctor_arg]), a nested foreign-symbol
   call, e.g. the [WithPrecision(precision)] in [call: "FromFormula"(expr,
   WithPrecision(precision))], or a list literal (the caller's own value for
   a [variadic] logical parameter, e.g.
   [mathkit.from_formula(.expr, [mathkit.with_precision(.digits)])]), or a
   map literal (the caller's own value for a [map[string]V] logical
   parameter, e.g. [mathkit.from_table({ "answer": 42 })]). A string not
   followed by "(" is an ordinary literal instead ([CaLit]). *)
and is_ident (t : Token.t) =
  match t.kind with Token.Ident _ -> true | _ -> false

and parse_call_arg st : Ast.call_arg =
  match (P.peek st).kind with
  | Token.Dot -> Ast.CaRef (parse_ref_path st)
  | Token.Str s -> (
      let t = P.advance st in
      match (P.peek st).kind with
      | Token.LParen -> Ast.CaCall (parse_nested_call st s t.span)
      | _ -> Ast.CaLit (Ast.LStr s, t.span))
  | Token.Int n ->
      let t = P.advance st in
      Ast.CaLit (Ast.LInt n, t.span)
  | Token.Float f ->
      let t = P.advance st in
      Ast.CaLit (Ast.LFloat f, t.span)
  | Token.Ident "type" when is_ident (P.peek_ahead st 1) ->
      (* "type" is contextual here, like in a handle declaration: a
         parameter named [type] stays a parameter when nothing follows it. *)
      let kw = P.advance st in
      let nt = P.advance st in
      let n = match nt.kind with Token.Ident n -> n | _ -> assert false in
      Ast.CaType (n, Span.merge kw.span nt.span)
  | Token.Ident n -> (
      let nt = P.advance st in
      match (P.peek st).kind with
      | Token.LBrace -> (
          match parse_ctor_arg st n nt.span with
          | Ast.ACtor c -> Ast.CaCtor c
          | _ -> assert false)
      | _ -> Ast.CaParam (n, nt.span))
  | Token.LBracket -> parse_call_list st
  | Token.LBrace -> parse_call_map st
  | _ ->
      P.error st (P.peek st).span "expected a call argument";
      ignore (P.advance st);
      Ast.CaParam ("", (P.peek st).span)

(* "[" call_arg ("," call_arg)* "]" — the caller has not consumed '[' yet. *)
and parse_call_list st : Ast.call_arg =
  let lb = P.advance st in
  (* '[' *)
  match (P.peek st).kind with
  | Token.RBracket ->
      let rb = P.advance st in
      Ast.CaList ([], Span.merge lb.span rb.span)
  | _ ->
      let first = parse_call_arg st in
      let rec more acc =
        match (P.peek st).kind with
        | Token.Comma ->
            ignore (P.advance st);
            if (P.peek st).kind = Token.RBracket then List.rev acc
            else more (parse_call_arg st :: acc)
        | _ -> List.rev acc
      in
      let items = more [ first ] in
      let close = P.expect st Token.RBracket "']' to close a list argument" in
      let finish = match close with Some t -> t.span | None -> lb.span in
      Ast.CaList (items, Span.merge lb.span finish)

(* "{" (string ":" call_arg ("," string ":" call_arg)* ","?)? "}" — the
   caller has not consumed '{' yet. A bare '{' in argument position can only
   open a map literal (a struct literal is always preceded by its name), so
   no lookahead is needed. Keys are string literals; a key written twice is
   diagnosed here, since every target's map would keep only one of them. *)
and parse_call_map st : Ast.call_arg =
  let lb = P.advance st in
  (* '{' *)
  let parse_entry () =
    let key =
      match (P.peek st).kind with
      | Token.Str s ->
          let t = P.advance st in
          Some (s, t.span)
      | _ ->
          P.error st (P.peek st).span "expected a string key in a map argument";
          None
    in
    ignore (P.expect st Token.Colon "':' after a map argument key");
    let value = parse_call_arg st in
    match key with Some (k, span) -> Some (k, span, value) | None -> None
  in
  let entries =
    match (P.peek st).kind with
    | Token.RBrace -> []
    | _ ->
        let first = parse_entry () in
        let rec more acc =
          match (P.peek st).kind with
          | Token.Comma ->
              ignore (P.advance st);
              if (P.peek st).kind = Token.RBrace then List.rev acc
              else more (parse_entry () :: acc)
          | _ -> List.rev acc
        in
        List.filter_map Fun.id (more [ first ])
  in
  List.iteri
    (fun i (k, span, _) ->
      if
        List.exists
          (fun (k', _, _) -> k' = k)
          (List.filteri (fun j _ -> j < i) entries)
      then
        P.error st span (Printf.sprintf "duplicate key %S in a map argument" k))
    entries;
  let close = P.expect st Token.RBrace "'}' to close a map argument" in
  let finish = match close with Some t -> t.span | None -> lb.span in
  Ast.CaMap (entries, Span.merge lb.span finish)

(* "string" "(" call_arg ("," call_arg)* ")" — the symbol has already been
   consumed at [symbol]/[symbol_span]; the cursor sits on "(". *)
and parse_nested_call st (symbol : string) (symbol_span : Span.span) :
    Ast.nested_call =
  let args = parse_call_args st in
  {
    Ast.nc_symbol = symbol;
    nc_symbol_span = symbol_span;
    nc_args = args;
    nc_span = symbol_span;
  }

(* "(" call_arg ("," call_arg)* ")" — the caller has not consumed '(' yet.
   Used both by [call_expr] below and directly by a language block's
   [call: "symbol"(args)] line, which has no "ns." prefix. *)
and parse_call_args st : Ast.call_arg list =
  ignore (P.expect st Token.LParen "'(' to open call arguments");
  match (P.peek st).kind with
  | Token.RParen ->
      ignore (P.advance st);
      []
  | _ ->
      let first = parse_call_arg st in
      let rec more acc =
        match (P.peek st).kind with
        | Token.Comma ->
            ignore (P.advance st);
            more (parse_call_arg st :: acc)
        | _ -> List.rev acc
      in
      let args = more [ first ] in
      ignore (P.expect st Token.RParen "')' to close call arguments");
      args

(* call_expr ::= "." name "(" call_arg ("," call_arg)* ")"  — [ns]/[ns_span]
   have already been consumed by the caller; the cursor sits on the '.'. *)
and parse_call_expr st ~(ns : string) ~(ns_span : Span.span) : Ast.call_expr =
  ignore (P.expect st Token.Dot "'.' in a call expression");
  let fn, fn_span =
    match (P.peek st).kind with
    | Token.Ident n ->
        let t = P.advance st in
        (n, t.span)
    | _ ->
        P.error st (P.peek st).span "expected a function name after '.'";
        ("", (P.peek st).span)
  in
  let args = parse_call_args st in
  {
    Ast.ce_ns = ns;
    ce_fn = fn;
    ce_head_span = Span.merge ns_span fn_span;
    ce_args = args;
    ce_span = Span.merge ns_span fn_span;
  }

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
      | Token.Dot -> Ast.ACall (parse_call_expr st ~ns:n ~ns_span:nt.span)
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

(* match ::= "match" ref "{" (pattern "=>" value)* "}"  — the selection table
   of an entry/config field, also reused as a [returns:] field value in the
   FFI ext/extern block. Patterns are literals (or "_"); a value is a field
   reference, a literal, or a stack of source traits resolved in place. *)
let parse_field_match st : Ast.field_match =
  let kw = P.advance st in
  (* 'match' *)
  let subject =
    match (P.peek st).kind with
    | Token.Dot -> parse_indexed_ref_path st
    | _ ->
        P.error st (P.peek st).span
          "expected a field reference (.name) as the match subject";
        { Ast.segs = []; index = None; ref_span = (P.peek st).span }
  in
  ignore (P.expect st Token.LBrace "'{' to open the match body");
  let parse_pattern () =
    let t = P.peek st in
    match t.kind with
    | Token.Str s ->
        ignore (P.advance st);
        (Ast.PString s, t.span)
    | Token.Int n ->
        ignore (P.advance st);
        (Ast.PInt n, t.span)
    | Token.Ident "_" ->
        ignore (P.advance st);
        (Ast.PWildcard, t.span)
    | Token.Ident "null" ->
        ignore (P.advance st);
        (Ast.PNull, t.span)
    | Token.Ident n ->
        ignore (P.advance st);
        (Ast.PName n, t.span)
    | _ ->
        P.error st t.span "expected a literal pattern or '_' in the match body";
        ignore (P.advance st);
        (Ast.PWildcard, t.span)
  in
  let parse_value () =
    let t = P.peek st in
    match t.kind with
    | Token.Dot ->
        let r = parse_ref_path st in
        if r.Ast.segs = [ "_" ] && r.Ast.index = None then
          (Ast.AVSubject r.Ast.ref_span, r.ref_span)
        else (Ast.AVRef r, r.ref_span)
    | Token.Str s ->
        ignore (P.advance st);
        (Ast.AVString s, t.span)
    | Token.Int n ->
        ignore (P.advance st);
        (Ast.AVInt n, t.span)
    | Token.Ident n ->
        ignore (P.advance st);
        (Ast.AVName n, t.span)
    | Token.At ->
        let traits = parse_trailing_traits st in
        let last =
          match List.rev traits with
          | (tr : Ast.trait) :: _ -> tr.tspan
          | [] -> t.span
        in
        (Ast.AVSources traits, Span.merge t.span last)
    | _ ->
        P.error st t.span
          "expected a field reference, a literal, or value sources after '=>'";
        ignore (P.advance st);
        (Ast.AVName "", t.span)
  in
  let rec arms acc =
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof -> List.rev acc
    | Token.Comma ->
        ignore (P.advance st);
        arms acc
    | _ ->
        let pat, pat_span = parse_pattern () in
        ignore
          (P.expect st Token.FatArrow "'=>' between the pattern and its value");
        let value, value_span = parse_value () in
        arms ({ Ast.pat; pat_span; value; value_span } :: acc)
  in
  let arms = arms [] in
  let close = P.expect st Token.RBrace "'}' to close the match body" in
  let finish = match close with Some c -> c.span | None -> kw.span in
  { Ast.subject; arms; match_span = Span.merge kw.span finish }
