(* Hand-written recursive-descent parser for the calculus, in the spirit of the
   .tono parser: it never raises; syntax errors become diagnostics and a parse
   error leaves an [EError] placeholder so checking can continue. Boundary types
   reuse the surface [Ast.ty] grammar. Precedence, tightest first:
     postfix '.'  >  unary '!' / negative literal  >  '* / %'  >  '+ - ++'
     >  comparisons  >  '&&'  >  '||'  >  '??' (right-assoc). *)

module T = Calc_token
module A = Calc_ast

type st = {
  toks : T.t array;
  mutable pos : int;
  mutable prev : Span.span; (* span of the most recently consumed token *)
  mutable diags : Diagnostic.t list;
  (* When set, a bare `name {` is a variable, not a struct ctor: a brace there
     opens a match/if block. A ctor in that position must be parenthesized. *)
  mutable no_ctor : bool;
}

let peek st = st.toks.(st.pos)
let kind st = (peek st).kind

let advance st =
  let t = peek st in
  st.prev <- t.span;
  if t.kind <> T.Eof then st.pos <- st.pos + 1;
  t

let error st span message =
  st.diags <-
    { Diagnostic.span; severity = Diagnostic.Error; message; code = None }
    :: st.diags

(* Report "expected X, found Y" at the current token. *)
let unexpected st what =
  error st (peek st).span
    (Printf.sprintf "expected %s, found %s" what (T.describe (kind st)))

(* [eat st k what] consumes the current token if it is [k]; otherwise diagnoses
   (without consuming) and returns false. *)
let eat st (k : T.kind) what =
  if kind st = k then (
    ignore (advance st);
    true)
  else (
    unexpected st what;
    false)

let node st ~start kind : A.expr = { A.kind; span = Span.merge start st.prev }

(* Run [f] with [no_ctor] set to [v], restoring it afterwards. *)
let with_no_ctor st v f =
  let old = st.no_ctor in
  st.no_ctor <- v;
  let r = f () in
  st.no_ctor <- old;
  r

let ident st what =
  match kind st with
  | T.Ident s ->
      ignore (advance st);
      s
  | _ ->
      unexpected st what;
      ""

(* ── Types (boundary) ──────────────────────────────────────────────────── *)

let rec parse_type st : Ast.ty =
  let base = parse_base_type st in
  let rec nullable t =
    if kind st = T.Question then
      let q = advance st in
      nullable (Ast.TNullable (t, q.span))
    else t
  in
  nullable base

and parse_base_type st : Ast.ty =
  let tok = peek st in
  match tok.kind with
  | T.Prim s ->
      ignore (advance st);
      Ast.TPrim (s, tok.span)
  | T.Ident s ->
      ignore (advance st);
      Ast.TName (s, [], tok.span)
  | T.LBracket ->
      ignore (advance st);
      ignore (eat st T.RBracket "']'");
      let inner = parse_type st in
      Ast.TList (inner, Span.merge tok.span st.prev)
  | T.KwMap ->
      ignore (advance st);
      ignore (eat st T.LBracket "'['");
      let k = parse_type st in
      ignore (eat st T.RBracket "']'");
      let v = parse_type st in
      Ast.TMap (k, v, Span.merge tok.span st.prev)
  | _ ->
      unexpected st "a type";
      Ast.TError tok.span

(* ── Expressions ───────────────────────────────────────────────────────── *)

let rec parse_expr st : A.expr = parse_coalesce st

and parse_coalesce st =
  let start = (peek st).span in
  let left = parse_or st in
  if kind st = T.QQuestion then (
    ignore (advance st);
    let right = parse_coalesce st in
    node st ~start (A.Coalesce (left, right)))
  else left

and parse_or st = parse_left st parse_and [ (T.PipePipe, A.Or) ]
and parse_and st = parse_left st parse_cmp [ (T.AmpAmp, A.And) ]

and parse_cmp st =
  parse_left st parse_add
    [
      (T.EqEq, A.Eq);
      (T.BangEq, A.Ne);
      (T.Lt, A.Lt);
      (T.Gt, A.Gt);
      (T.LtEq, A.Le);
      (T.GtEq, A.Ge);
    ]

and parse_add st =
  parse_left st parse_mul
    [ (T.Plus, A.Add); (T.Minus, A.Sub); (T.PlusPlus, A.Concat) ]

and parse_mul st =
  parse_left st parse_unary
    [ (T.Star, A.Mul); (T.Slash, A.Div); (T.Percent, A.Mod) ]

(* Left-associative binary level: parse [sub], then fold any operator in [ops]. *)
and parse_left st sub ops =
  let start = (peek st).span in
  let rec loop left =
    match List.assoc_opt (kind st) ops with
    | Some op ->
        ignore (advance st);
        let right = sub st in
        loop (node st ~start (A.Binop (op, left, right)))
    | None -> left
  in
  loop (sub st)

and parse_unary st =
  let start = (peek st).span in
  match kind st with
  | T.Bang ->
      ignore (advance st);
      let e = parse_unary st in
      node st ~start (A.Not e)
  | T.Minus -> (
      ignore (advance st);
      match kind st with
      | T.Int s ->
          ignore (advance st);
          node st ~start (A.Lit (A.Int ("-" ^ s)))
      | T.Float f ->
          ignore (advance st);
          node st ~start (A.Lit (A.Float (-.f)))
      | _ ->
          unexpected st "a numeric literal after '-'";
          node st ~start A.EError)
  | _ -> parse_postfix st

and parse_postfix st =
  let start = (peek st).span in
  let rec loop e =
    if kind st = T.Dot then (
      ignore (advance st);
      let field = ident st "a field name" in
      loop (node st ~start (A.Field (e, field))))
    else e
  in
  loop (parse_atom st)

and parse_atom st : A.expr =
  let start = (peek st).span in
  match kind st with
  | T.Int s ->
      ignore (advance st);
      node st ~start (A.Lit (A.Int s))
  | T.Float f ->
      ignore (advance st);
      node st ~start (A.Lit (A.Float f))
  | T.Str s ->
      ignore (advance st);
      node st ~start (A.Lit (A.Str s))
  | T.KwTrue ->
      ignore (advance st);
      node st ~start (A.Lit (A.Bool true))
  | T.KwFalse ->
      ignore (advance st);
      node st ~start (A.Lit (A.Bool false))
  | T.KwNone ->
      ignore (advance st);
      node st ~start A.None_
  | T.KwSome ->
      ignore (advance st);
      ignore (eat st T.LParen "'('");
      let e = parse_expr st in
      ignore (eat st T.RParen "')'");
      node st ~start (A.Some_ e)
  | T.KwIf ->
      ignore (advance st);
      let c = with_no_ctor st true (fun () -> parse_expr st) in
      ignore (eat st T.KwThen "'then'");
      let t = parse_expr st in
      ignore (eat st T.KwElse "'else'");
      let e = parse_expr st in
      node st ~start (A.If (c, t, e))
  | T.KwLet ->
      ignore (advance st);
      let x = ident st "a binding name" in
      ignore (eat st T.Eq "'='");
      let v = parse_expr st in
      ignore (eat st T.KwIn "'in'");
      let body = parse_expr st in
      node st ~start (A.Let (x, v, body))
  | T.KwMatch ->
      ignore (advance st);
      let scrut = with_no_ctor st true (fun () -> parse_expr st) in
      ignore (eat st T.LBrace "'{'");
      let arms = parse_arms st in
      ignore (eat st T.RBrace "'}'");
      node st ~start (A.Match (scrut, arms))
  | T.KwMap | T.KwFilter ->
      let is_map = kind st = T.KwMap in
      ignore (advance st);
      ignore (eat st T.LParen "'('");
      let e = parse_expr st in
      ignore (eat st T.Comma "','");
      let fn = ident st "a function name" in
      ignore (eat st T.RParen "')'");
      node st ~start (if is_map then A.Map (e, fn) else A.Filter (e, fn))
  | T.KwFold ->
      ignore (advance st);
      ignore (eat st T.LParen "'('");
      let e = parse_expr st in
      ignore (eat st T.Comma "','");
      let init = parse_expr st in
      ignore (eat st T.Comma "','");
      let fn = ident st "a function name" in
      ignore (eat st T.RParen "')'");
      node st ~start (A.Fold (e, init, fn))
  | T.LParen ->
      ignore (advance st);
      let e = with_no_ctor st false (fun () -> parse_expr st) in
      ignore (eat st T.RParen "')'");
      e
  | T.Ident name -> (
      ignore (advance st);
      match kind st with
      | T.LParen ->
          let args = parse_args st in
          node st ~start (A.Call (name, args))
      | T.LBrace when not st.no_ctor ->
          let fields = parse_ctor_fields st in
          node st ~start (A.Ctor (name, fields))
      | _ -> node st ~start (A.Var name))
  | _ ->
      unexpected st "an expression";
      ignore (advance st);
      node st ~start A.EError

and parse_args st : A.expr list =
  ignore (eat st T.LParen "'('");
  if kind st = T.RParen then (
    ignore (advance st);
    [])
  else
    let rec loop acc =
      let e = with_no_ctor st false (fun () -> parse_expr st) in
      if kind st = T.Comma then (
        ignore (advance st);
        loop (e :: acc))
      else (
        ignore (eat st T.RParen "')'");
        List.rev (e :: acc))
    in
    loop []

and parse_ctor_fields st : (string * A.expr) list =
  ignore (eat st T.LBrace "'{'");
  if kind st = T.RBrace then (
    ignore (advance st);
    [])
  else
    let field () =
      let name = ident st "a field name" in
      ignore (eat st T.Colon "':'");
      let e = with_no_ctor st false (fun () -> parse_expr st) in
      (name, e)
    in
    let rec loop acc =
      let f = field () in
      if kind st = T.Comma then (
        ignore (advance st);
        loop (f :: acc))
      else (
        ignore (eat st T.RBrace "'}'");
        List.rev (f :: acc))
    in
    loop []

and parse_arms st : (A.pattern * A.expr) list =
  let arm () =
    let pat =
      match kind st with
      | T.KwUnknown ->
          ignore (advance st);
          ignore (eat st T.LParen "'('");
          let bind = ident st "a binding name" in
          ignore (eat st T.RParen "')'");
          A.PUnknown { bind }
      | T.Ident variant ->
          ignore (advance st);
          if kind st = T.LParen then (
            ignore (advance st);
            let bind = ident st "a binding name" in
            ignore (eat st T.RParen "')'");
            A.PVariant { variant; bind = Some bind })
          else A.PVariant { variant; bind = None }
      | _ ->
          unexpected st "a pattern";
          ignore (advance st);
          A.PVariant { variant = ""; bind = None }
    in
    ignore (eat st T.FatArrow "'=>'");
    let body = with_no_ctor st false (fun () -> parse_expr st) in
    (pat, body)
  in
  let rec loop acc =
    if kind st = T.RBrace || kind st = T.Eof then List.rev acc
    else loop (arm () :: acc)
  in
  loop []

(* ── Functions and program ─────────────────────────────────────────────── *)

let parse_params st : (string * Ast.ty) list =
  if kind st = T.RParen then []
  else
    let param () =
      let name = ident st "a parameter name" in
      ignore (eat st T.Colon "':'");
      let ty = parse_type st in
      (name, ty)
    in
    let rec loop acc =
      let p = param () in
      if kind st = T.Comma then (
        ignore (advance st);
        loop (p :: acc))
      else List.rev (p :: acc)
    in
    loop []

let parse_fn_def st : A.fn_def =
  ignore (eat st T.KwFn "'fn'");
  let name_tok = peek st in
  let name = ident st "a function name" in
  ignore (eat st T.LParen "'('");
  let params = parse_params st in
  ignore (eat st T.RParen "')'");
  ignore (eat st T.Arrow "'->'");
  let ret = parse_type st in
  ignore (eat st T.Eq "'='");
  let body = parse_expr st in
  { A.name; name_span = name_tok.span; params; ret; body }

let parse (src : string) : A.program * Diagnostic.t list =
  let toks, lex_diags = Calc_lexer.tokenize src in
  (* The lexer always appends an [Eof] token, so the array is never empty. *)
  let arr = Array.of_list toks in
  let st =
    { toks = arr; pos = 0; prev = arr.(0).span; diags = []; no_ctor = false }
  in
  let rec loop acc =
    if kind st = T.Eof then List.rev acc
    else
      let before = st.pos in
      let fn = parse_fn_def st in
      (* Defensive: [parse_fn_def] always reaches [parse_atom], which consumes a
         token for any non-Eof input, so the cursor cannot actually stick here;
         the guard just rules out an infinite loop if that ever changes. *)
      if st.pos = before then ignore (advance st);
      loop (fn :: acc)
  in
  let program = loop [] in
  (program, lex_diags @ List.rev st.diags)
