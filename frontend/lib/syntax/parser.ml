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

(* ── Traits ────────────────────────────────────────────────────────────── *)

(* Trait and reference parsing lives in [Parser_traits]; aliased locally. *)
let parse_trait = Parser_traits.parse_trait
let parse_trailing_traits = Parser_traits.parse_trailing_traits

(* ── Members ───────────────────────────────────────────────────────────── *)

(* Selection-table (match) parsing lives in [Parser_traits] so [Parser_extern]
   can reuse it for a [returns:] field value; aliased locally. *)
let parse_field_match = Parser_traits.parse_field_match

(* member ::= name ":" type trait* ("=" (match | call_expr))? trait*
   A trait may appear before the value, after it, or split across both (the
   two lists are concatenated in source order into one [mtraits]): a source
   marker paired with a call value reads naturally either way ("@with = ns.fn(...)"
   marks the field injectable before showing its construction fallback;
   "= ns.fn(...) @with" shows the fallback first), and the two spellings are
   otherwise indistinguishable once parsed. The printer always emits the
   trailing spelling; a member written with a leading trait round-trips
   through [fmt] into that canonical form rather than back to itself. *)
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
  let leading_traits = parse_trailing_traits st in
  let mvalue =
    match (P.peek st).kind with
    | Token.Eq -> (
        ignore (P.advance st);
        match (P.peek st).kind with
        | Token.Ident "match" -> Some (Ast.MMatch (parse_field_match st))
        | Token.Ident ns -> (
            match (P.peek_ahead st 1).kind with
            | Token.Dot ->
                let nst = P.advance st in
                Some
                  (Ast.MCall
                     (Parser_traits.parse_call_expr st ~ns ~ns_span:nst.span))
            | _ ->
                P.error st (P.peek st).span
                  "expected 'match' or a 'namespace.fn(...)' call after '='";
                None)
        | _ ->
            P.error st (P.peek st).span
              "expected 'match' or a 'namespace.fn(...)' call after '='";
            None)
    | _ -> None
  in
  let trailing_traits = parse_trailing_traits st in
  {
    Ast.mname = name;
    mname_span = nt.span;
    mtype = ty;
    mvalue;
    mtraits = leading_traits @ trailing_traits;
  }

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
  Parser_extern.check_not_error_name st "union" name nt.span;
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

(* op_impl ::= "impl" "." name ("." name)+ "(" call_arg ("," call_arg)* ")"
   — an op's own bespoke body: a call into a declared opaque
   handle's method. "impl" is a contextual keyword here (only recognized
   right after an op's traits, exactly like "impl" after "ext" in the legacy
   grammar); a field or parameter genuinely named "impl" is not ambiguous in
   this position since nothing else can start an op's trailing clause with
   that identifier. The receiver is every segment but the last (".bus" in
   ".bus.send(...)"); the last is the method name. At least two segments are
   required: "impl .send(...)" alone has no receiver to resolve the method
   against. *)
let parse_op_impl st : Ast.op_impl option =
  match (P.peek st).kind with
  | Token.Ident "impl" -> (
      let impl_t = P.advance st in
      let rec segs acc =
        match (P.peek st).kind with
        | Token.Dot -> (
            ignore (P.advance st);
            match (P.peek st).kind with
            | Token.Ident n ->
                let t = P.advance st in
                segs ((n, t.span) :: acc)
            | _ ->
                P.error st (P.peek st).span "expected a field name after '.'";
                List.rev acc)
        | _ -> List.rev acc
      in
      match segs [] with
      | [] ->
          P.error st (P.peek st).span
            "expected a field reference after 'impl', e.g. 'impl \
             .bus.send(...)'";
          None
      | [ (_, span) ] ->
          P.error st span
            "'impl' needs a receiver before the method, e.g. 'impl \
             .bus.send(...)' rather than 'impl .send(...)'";
          None
      | all -> (
          let recv, (method_, method_span) =
            match List.rev all with
            | last :: rest -> (List.rev rest, last)
            | [] -> assert false
          in
          let recv_span =
            match recv with
            | (_, first_span) :: _ -> Span.merge first_span method_span
            | [] -> method_span
          in
          match (P.peek st).kind with
          | Token.LParen ->
              let args = Parser_traits.parse_call_args st in
              Some
                {
                  Ast.oi_recv =
                    {
                      Ast.segs = List.map fst recv;
                      index = None;
                      ref_span = recv_span;
                    };
                  oi_method = method_;
                  oi_method_span = method_span;
                  oi_args = args;
                  oi_span = Span.merge impl_t.span method_span;
                }
          | _ ->
              P.error st (P.peek st).span
                "expected '(' after 'impl .field.method'";
              None))
  | _ -> None

(* op ::= "op" name "(" (name ":" type)? ")" ( ":" type )? op_trait*
   ("impl" ...)?  — the parameter, when present, must name itself (".param"
   needs a name to give its reference provenance); the output type is
   optional, errors are carried by a trailing "@errors(...)" trait, and the
   op's own bespoke body (if any) is [parse_op_impl] above. *)
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
  let pname =
    match ((P.peek st).kind, (P.peek_ahead st 1).kind) with
    | Token.Ident n, Token.Colon ->
        ignore (P.advance st);
        (* param name *)
        ignore (P.advance st);
        (* ':' *)
        Some n
    | Token.RParen, _ -> None
    | _ ->
        P.error st (P.peek st).span
          "operation parameter must be named, e.g. 'op foo(note_ref: NoteRef)' \
           instead of 'op foo(NoteRef)'";
        None
  in
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
  let oimpl = parse_op_impl st in
  {
    Ast.dname = name;
    dname_span = nt.span;
    pub;
    dtraits;
    dkind = Ast.DOp { pname; input; output; oimpl };
  }

(* ── Structs ───────────────────────────────────────────────────────────── *)

(* The struct body: members interleaved with op declarations (a struct with ops
   is an entry). Op traits trail the op line, so a trait written between an op
   and the next item binds to the op, exactly as at the top level. *)
let parse_struct_items st : Ast.member list * Ast.decl list =
  let rec go members ops =
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof -> (List.rev members, List.rev ops)
    | Token.Ident _ -> go (parse_member st :: members) ops
    | Token.KwOp -> go members (parse_op st ~pub:false ~dtraits:[] :: ops)
    | Token.Comma ->
        ignore (P.advance st);
        go members ops
    | _ ->
        P.error st (P.peek st).span
          (Printf.sprintf "unexpected %s in struct body"
             (Token.describe (P.peek st).kind));
        ignore (P.advance st);
        go members ops
  in
  go [] []

(* struct ::= "struct" name generics? "{" (member | op)* "}" *)
let parse_struct st ~pub ~dtraits : Ast.decl =
  ignore (P.advance st);
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
  Parser_extern.check_not_error_name st "struct" name nt.span;
  let params = parse_generics st in
  ignore (P.expect st Token.LBrace "'{' to open the struct body");
  let members, ops = parse_struct_items st in
  ignore (P.expect st Token.RBrace "'}' to close the struct body");
  {
    Ast.dname = name;
    dname_span = nt.span;
    pub;
    dtraits;
    dkind = Ast.DStruct { params; members; ops };
  }

(* ── Extensions ────────────────────────────────────────────────────────── *)

(* The legacy [ext hook|contract|constraint|impl] grammar lives in
   [Parser_ext]; the new [ext <name> { ... }] library-block grammar lives in
   [Parser_extern]. *)
let parse_ext = Parser_ext.parse_ext ~parse_type

(* Consumes a brace-balanced block, assuming the current token is its opening
   '{'. Used to recover from a body that was never going to parse (a reserved
   ext-kind word used as a library name): skipping straight to the matching
   '}' avoids re-entering a parser that would otherwise cascade through the
   mismatched body one confusing diagnostic at a time. *)
let skip_balanced_braces st : unit =
  ignore (P.expect st Token.LBrace "'{' to open the block");
  let rec go depth =
    if depth = 0 || P.at_eof st then ()
    else
      match (P.peek st).kind with
      | Token.LBrace ->
          ignore (P.advance st);
          go (depth + 1)
      | Token.RBrace ->
          ignore (P.advance st);
          go (depth - 1)
      | _ ->
          ignore (P.advance st);
          go depth
  in
  go 1

(* ── Declarations and files ────────────────────────────────────────────── *)

(* decl ::= trait* "pub"? (struct | union | enum | op | ext). Returns [None] when
   the keyword is missing so the file loop can resynchronize. *)
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
  | Token.KwExt -> (
      (* "ext hook|contract|constraint|impl ..." is the legacy grammar
         (Parser_ext, untouched below); any other identifier after "ext" is
         the library name of the new "ext <name> { ... }" FFI block
         (Parser_extern). One token of lookahead beyond "ext" disambiguates
         without consuming it for the legacy path, which advances "ext"
         itself. *)
      match (P.peek_ahead st 1).kind with
      | Token.Ident (("hook" | "contract" | "constraint" | "impl") as kind) -> (
          (* The legacy grammar always requires a name after the kind word
             (ext <kind> <name> ...); a '{' immediately after it can only be
             a mistyped legacy form, never a valid one. Name the collision
             once, up front, then skip straight past the whole malformed
             body instead of letting legacy parsing re-enter it and cascade
             into a diagnostic per token while it fails to recover. *)
          match (P.peek_ahead st 2).kind with
          | Token.LBrace ->
              P.error st (P.peek_ahead st 1).span
                (Printf.sprintf
                   "'%s' is a reserved ext-kind word here, not a library name: \
                    a library cannot currently be named '%s'"
                   kind kind);
              ignore (P.advance st);
              (* 'ext' *)
              ignore (P.advance st);
              (* kind word *)
              skip_balanced_braces st;
              None
          | _ -> Some (parse_ext st ~pub ~dtraits))
      | Token.Prim s ->
          (* A primitive type name can never be a legal library name (every
             foreign form the ext block declares shares the tono type
             vocabulary), unlike the kind-word case above, which is only a
             collision when a legacy '{' follows. This one is unconditional,
             so name it and recover the same way: skip the block when one is
             there to skip. *)
          P.error st (P.peek_ahead st 1).span
            (Printf.sprintf
               "'%s' is a reserved primitive type name, not a library name: a \
                library cannot currently be named '%s'"
               s s);
          (match (P.peek_ahead st 2).kind with
          | Token.LBrace ->
              ignore (P.advance st);
              (* 'ext' *)
              ignore (P.advance st);
              (* primitive name *)
              skip_balanced_braces st
          | _ ->
              ignore (P.advance st);
              ignore (P.advance st));
          None
      | Token.Ident n ->
          ignore (P.advance st);
          (* 'ext' *)
          let nt = P.advance st in
          (* library name *)
          Some
            (Parser_extern.parse_ext_lib ~parse_type
               ~parse_type_no_error:
                 (Parser_extern.parse_type_no_error ~parse_type)
               st ~pub ~dtraits ~name:n ~name_span:nt.span)
      | _ ->
          P.error st (P.peek_ahead st 1).span
            "expected an extension kind or a library name after 'ext'";
          ignore (P.advance st);
          None)
  | Token.KwTest ->
      (* A test is neither exported nor decorated: it is not a shape. *)
      if pub then
        P.error st (P.peek st).span "a test declaration cannot be 'pub'";
      (match dtraits with
      | { Ast.tspan; _ } :: _ ->
          P.error st tspan "a test declaration carries no traits"
      | [] -> ());
      Some (Parser_tests.parse_test st)
  | _ ->
      P.error st (P.peek st).span
        (Printf.sprintf
           "expected a declaration (struct, enum, union, op, or ext), found %s"
           (Token.describe (P.peek st).kind));
      None

(* import ::= "import" segment ("." segment)* ("as" alias)?  — the dotted path
   names the target module; the qualifier used in references is the alias when
   present, otherwise the last segment. *)
let parse_import st : Ast.import =
  let kw = P.advance st in
  (* 'import' *)
  let segment () =
    match (P.peek st).kind with
    | Token.Ident s ->
        let t = P.advance st in
        (s, t.span)
    | _ ->
        P.error st (P.peek st).span "expected a module path segment";
        ("", (P.peek st).span)
  in
  let first, fspan = segment () in
  let rec more acc last_span =
    match (P.peek st).kind with
    | Token.Dot ->
        ignore (P.advance st);
        let s, sp = segment () in
        more (s :: acc) sp
    | _ -> (List.rev acc, last_span)
  in
  let path, path_end = more [ first ] fspan in
  let alias, alias_end =
    match (P.peek st).kind with
    | Token.KwAs -> (
        ignore (P.advance st);
        match (P.peek st).kind with
        | Token.Ident a ->
            let t = P.advance st in
            (Some a, t.span)
        | _ ->
            P.error st (P.peek st).span "expected an alias name after 'as'";
            (None, path_end))
    | _ -> (None, path_end)
  in
  { Ast.imported_path = path; alias; ispan = Span.merge kw.span alias_end }

(* A top-level item can start with an import, a trait, [pub], or one of the shape
   keywords; resynchronization skips to the next such token. *)
let is_decl_start = function
  | Token.At | Token.KwImport | Token.KwPub | Token.KwStruct | Token.KwUnion
  | Token.KwEnum | Token.KwOp | Token.KwExt | Token.KwTest ->
      true
  | _ -> false

let parse_file st : Ast.file =
  let rec go imports decls =
    if P.at_eof st then
      { Ast.imports = List.rev imports; decls = List.rev decls }
    else
      match (P.peek st).kind with
      | Token.KwImport -> go (parse_import st :: imports) decls
      | _ -> (
          match parse_decl st with
          | Some d -> go imports (d :: decls)
          | None ->
              (* parse_decl already diagnosed; ensure progress, then skip to the
                 next top-level boundary. *)
              if not (P.at_eof st) then ignore (P.advance st);
              while
                (not (P.at_eof st)) && not (is_decl_start (P.peek st).kind)
              do
                ignore (P.advance st)
              done;
              go imports decls)
  in
  go [] []

let parse (src : string) : Ast.file * Diagnostic.t list =
  let toks, lex_diags = Lexer.tokenize src in
  let st = P.create toks in
  let file = parse_file st in
  (file, Diagnostic.sort (lex_diags @ P.diagnostics st))
