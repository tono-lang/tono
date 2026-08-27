(* The type grammar: a closed, mutually recursive group with no dependency on
   the declaration parsers, so every other parser module can call it
   directly. *)

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

(* name, name '[' args ']', or a qualified 'qualifier.Name' (optionally applied).
   A '.' after the first identifier marks a cross-module reference: the first
   segment is the import qualifier and the second is the shape name. *)
and parse_named st t name =
  ignore (P.advance st);
  (* name (the first identifier) *)
  match (P.peek st).kind with
  | Token.Dot -> (
      ignore (P.advance st);
      (* '.' *)
      match (P.peek st).kind with
      | Token.Ident tyname ->
          let nt = P.advance st in
          let args, finish =
            parse_opt_generics st (Span.merge t.span nt.span)
          in
          Ast.TQName (name, tyname, args, finish)
      | _ ->
          P.error st (P.peek st).span
            "expected a type name after the '.' module qualifier";
          Ast.TError (Span.merge t.span (P.peek st).span))
  | _ ->
      let args, finish = parse_opt_generics st t.span in
      Ast.TName (name, args, finish)

(* An optional '[' type (',' type)* ']' generic application after a type head;
   returns the argument types and the span extended to the closing bracket. *)
and parse_opt_generics st base_span : Ast.ty list * Span.span =
  match (P.peek st).kind with
  | Token.LBracket ->
      ignore (P.advance st);
      (* '[' *)
      let args = parse_type_list st in
      let close = P.expect st Token.RBracket "']' to close generic arguments" in
      let finish = match close with Some c -> c.span | None -> base_span in
      (args, Span.merge base_span finish)
  | _ -> ([], base_span)

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

(* generics ::= "[" name ("," name)* "]"  — the type parameters a struct or
   union declares after its name, as opposed to the type arguments
   [parse_opt_generics] applies to a type head. *)
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
