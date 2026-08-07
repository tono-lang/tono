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
let parse_ref_path = Parser_traits.parse_ref_path
let parse_trait = Parser_traits.parse_trait
let parse_trailing_traits = Parser_traits.parse_trailing_traits

(* ── Members ───────────────────────────────────────────────────────────── *)

(* match ::= "match" ref "{" (pattern "=>" value)* "}"  — the selection table of
   an entry/config field. Patterns are literals (or "_"); a value is a field
   reference, a literal, or a stack of source traits resolved in place. *)
let parse_field_match st : Ast.field_match =
  let kw = P.advance st in
  (* 'match' *)
  let subject =
    match (P.peek st).kind with
    | Token.Dot -> parse_ref_path st
    | _ ->
        P.error st (P.peek st).span
          "expected a field reference (.name) as the match subject";
        { Ast.segs = []; ref_span = (P.peek st).span }
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
        (Ast.AVRef r, r.ref_span)
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

(* member ::= name ":" type ("=" match)? trait* *)
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
  let mmatch =
    match (P.peek st).kind with
    | Token.Eq -> (
        ignore (P.advance st);
        match (P.peek st).kind with
        | Token.Ident "match" -> Some (parse_field_match st)
        | _ ->
            P.error st (P.peek st).span
              "expected 'match' after '='; selection is the only field \
               expression";
            None)
    | _ -> None
  in
  let traits = parse_trailing_traits st in
  {
    Ast.mname = name;
    mname_span = nt.span;
    mtype = ty;
    mmatch;
    mtraits = traits;
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

(* op ::= "op" name "(" (name ":")? type? ")" ( ":" type )? op_trait*  — the
   parameter name is optional (the legacy unnamed form still parses), the
   output type is optional, and errors are carried by a trailing
   "@errors(...)" trait. *)
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
    | _ -> None
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
  {
    Ast.dname = name;
    dname_span = nt.span;
    pub;
    dtraits;
    dkind = Ast.DOp { pname; input; output };
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
let parse_ext_sig st : Ast.ext_sig =
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
let parse_ext st ~pub ~dtraits : Ast.decl =
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
    | Token.LParen -> Some (parse_ext_sig st)
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
  | Token.KwExt -> Some (parse_ext st ~pub ~dtraits)
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
