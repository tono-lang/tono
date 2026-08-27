(* Hand-written recursive-descent parser: one function per grammar nonterminal,
   single-token lookahead. It builds the surface AST and accumulates diagnostics;
   it never raises. *)

module P = Parser_state

(* ── Types ─────────────────────────────────────────────────────────────── *)

(* The type grammar lives in [Parser_type]; aliased locally. *)
let parse_type = Parser_type.parse_type

(* ── Traits ────────────────────────────────────────────────────────────── *)

(* Trait and reference parsing lives in [Parser_traits]; aliased locally. A
   trait on a line of its own belongs to the declaration or body item after
   it ([parse_leading_traits], at every item start); a trait continuing a
   line belongs to that line ([parse_inline_traits], after a signature, a
   member, a case, a variant, or a union head). *)
let parse_trait = Parser_traits.parse_trait
let parse_leading_traits = Parser_traits.parse_leading_traits
let parse_inline_traits = Parser_traits.parse_inline_traits

(* ── Members ───────────────────────────────────────────────────────────── *)

(* Selection-table (match) parsing lives in [Parser_traits] so [Parser_extern]
   can reuse it for a [returns:] field value; aliased locally. *)
let parse_field_match = Parser_traits.parse_field_match

(* handle_call ::= "." name ("." name)+ "(" call_arg ("," call_arg)* ")"
   — a call into a declared opaque handle's method, shared by an op's own
   "impl" body and a member's "= .h.m(...)" value source. The cursor sits on
   the first '.'; [what] names the surrounding form in diagnostics ("impl"
   or "'='") and [head_span] is the span the whole call is merged from (the
   "impl" keyword's, or the '=' sign's). The receiver is every segment but
   the last (".bus" in ".bus.send(...)"); the last is the method name. At
   least two segments are required: ".send(...)" alone has no receiver to
   resolve the method against. *)
let parse_handle_call st ~what ~head_span : Ast.op_impl option =
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
        (Printf.sprintf
           "expected a field reference after %s, e.g. '%s .bus.send(...)'" what
           what);
      None
  | [ (_, span) ] ->
      P.error st span
        (Printf.sprintf
           "%s needs a receiver before the method, e.g. '%s .bus.send(...)' \
            rather than '%s .send(...)'"
           what what what);
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
              oi_span = Span.merge head_span method_span;
            }
      | _ ->
          P.error st (P.peek st).span
            (Printf.sprintf "expected '(' after '%s .field.method'" what);
          None)

(* member ::= name ":" type trait* ("=" (match | call_expr | handle_call))?
   trait*
   [leading] are the traits written on their own lines above the member. On
   the member's line a trait may appear before the value, after it, or split
   across both (the lists are concatenated in source order into one
   [mtraits]): a source marker paired with a call value reads naturally
   either way ("@with = ns.fn(...)" marks the field injectable before showing
   its construction fallback; "= ns.fn(...) @with" shows the fallback first),
   and the spellings are indistinguishable once parsed. The printer always
   emits the inline trailing spelling; a member written otherwise round-trips
   through [fmt] into that canonical form rather than back to itself. *)
let parse_member st ~leading : Ast.member =
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
  let head_traits = parse_inline_traits st in
  let mvalue =
    match (P.peek st).kind with
    | Token.Eq -> (
        let eq_t = P.advance st in
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
                  "expected 'match', a 'namespace.fn(...)' call, or a \
                   '.handle.method(...)' call after '='";
                None)
        (* A leading '.' is a handle method call: the receiver is a sibling
           field (a declared opaque handle), the value is what its method
           returns. *)
        | Token.Dot ->
            Option.map
              (fun hc -> Ast.MHandleCall hc)
              (parse_handle_call st ~what:"'='" ~head_span:eq_t.span)
        | _ ->
            P.error st (P.peek st).span
              "expected 'match', a 'namespace.fn(...)' call, or a \
               '.handle.method(...)' call after '='";
            None)
    | _ -> None
  in
  let trailing_traits = parse_inline_traits st in
  {
    Ast.mname = name;
    mname_span = nt.span;
    mtype = ty;
    mvalue;
    mtraits = leading @ head_traits @ trailing_traits;
  }

(* ── Sums ──────────────────────────────────────────────────────────────── *)

(* Union and enum parsing lives in [Parser_sum]; aliased locally. *)
let parse_union = Parser_sum.parse_union
let parse_enum = Parser_sum.parse_enum

(* ── Operations ────────────────────────────────────────────────────────── *)

(* op_impl ::= "impl" handle_call — an op's own bespoke body. "impl" is a
   contextual keyword here (only recognized right after an op's traits,
   exactly like "impl" after "ext" in the legacy grammar); a field or
   parameter genuinely named "impl" is not ambiguous in this position since
   nothing else can start an op's trailing clause with that identifier. *)
let parse_op_impl st : Ast.op_impl option =
  match (P.peek st).kind with
  | Token.Ident "impl" ->
      let impl_t = P.advance st in
      parse_handle_call st ~what:"impl" ~head_span:impl_t.span
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
  (* Traits continuing the signature line join the ones written above the
     op; lowering lifts @errors into Operation.errors and bags the rest. *)
  let dtraits = dtraits @ parse_inline_traits st in
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
   is an entry). A trait on its own line belongs to the member or op after
   it, exactly as at the top level; only a trait continuing a line stays with
   that line. *)
let parse_struct_items st :
    Ast.member list * Ast.decl list * Ast.lang_block list =
  let starts_item = function
    | Token.Ident _ | Token.KwOp -> true
    | _ -> false
  in
  let rec go members ops langs =
    let leading =
      Parser_traits.parse_item_traits st ~what:"member or op" ~starts_item
    in
    match (P.peek st).kind with
    | Token.RBrace | Token.Eof ->
        (List.rev members, List.rev ops, List.rev langs)
    | Token.Ident _ when (P.peek_ahead st 1).kind = Token.LBrace ->
        go members ops
          (Parser_extern.parse_struct_lang_block st ~traits:leading :: langs)
    | Token.Ident _ -> go (parse_member st ~leading :: members) ops langs
    | Token.KwOp ->
        go members (parse_op st ~pub:false ~dtraits:leading :: ops) langs
    | Token.Comma ->
        ignore (P.advance st);
        go members ops langs
    | _ ->
        P.error st (P.peek st).span
          (Printf.sprintf "unexpected %s in struct body"
             (Token.describe (P.peek st).kind));
        ignore (P.advance st);
        go members ops langs
  in
  go [] [] []

(* struct ::= "struct" name generics? "{" (member | op | lang_block)* "}" *)
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
  let params = Parser_type.parse_generics st in
  ignore (P.expect st Token.LBrace "'{' to open the struct body");
  let members, ops, slangs = parse_struct_items st in
  ignore (P.expect st Token.RBrace "'}' to close the struct body");
  {
    Ast.dname = name;
    dname_span = nt.span;
    pub;
    dtraits;
    dkind = Ast.DStruct { params; members; ops; slangs };
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
  let dtraits = parse_leading_traits st in
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
