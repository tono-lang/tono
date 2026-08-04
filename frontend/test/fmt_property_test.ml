open Tono_frontend
module G = QCheck.Gen

(* Applicative sugar so generators read like the records they build. *)
let ( let+ ) x f = G.map f x
let ( and+ ) a b = G.pair a b
let dpos : Span.pos = { line = 0; col = 0; offset = 0 }
let dspan : Span.span = { start = dpos; finish = dpos }

(* ── Span erasure: compare parsed and generated ASTs structurally ───────── *)

let rec erase_ty = function
  | Ast.TPrim (p, _) -> Ast.TPrim (p, dspan)
  | Ast.TName (n, args, _) -> Ast.TName (n, List.map erase_ty args, dspan)
  | Ast.TQName (q, n, args, _) ->
      Ast.TQName (q, n, List.map erase_ty args, dspan)
  | Ast.TList (e, _) -> Ast.TList (erase_ty e, dspan)
  | Ast.TMap (k, v, _) -> Ast.TMap (erase_ty k, erase_ty v, dspan)
  | Ast.TNullable (t, _) -> Ast.TNullable (erase_ty t, dspan)
  | Ast.TError _ -> Ast.TError dspan

let erase_ref (r : Ast.ref_path) = { r with Ast.ref_span = dspan }

(* A trait argument can carry a reference, which carries a span of its own. *)
let rec erase_arg = function
  | Ast.ARef r -> Ast.ARef (erase_ref r)
  | Ast.AKv (k, v) -> Ast.AKv (k, erase_arg v)
  | (Ast.AString _ | Ast.AInt _ | Ast.AFloat _ | Ast.AName _) as a -> a

let erase_trait (t : Ast.trait) =
  { t with Ast.tspan = dspan; targs = List.map erase_arg t.Ast.targs }

let erase_arm_value = function
  | Ast.AVRef r -> Ast.AVRef (erase_ref r)
  | Ast.AVSources ts -> Ast.AVSources (List.map erase_trait ts)
  | (Ast.AVString _ | Ast.AVInt _ | Ast.AVName _) as v -> v

let erase_match (fm : Ast.field_match) =
  {
    Ast.subject = erase_ref fm.Ast.subject;
    arms =
      List.map
        (fun (a : Ast.match_arm) ->
          {
            a with
            Ast.pat_span = dspan;
            value = erase_arm_value a.Ast.value;
            value_span = dspan;
          })
        fm.Ast.arms;
    match_span = dspan;
  }

let erase_member (m : Ast.member) =
  {
    m with
    Ast.mname_span = dspan;
    mtype = erase_ty m.Ast.mtype;
    mmatch = Option.map erase_match m.Ast.mmatch;
    mtraits = List.map erase_trait m.Ast.mtraits;
  }

let erase_case (c : Ast.enum_case) =
  {
    c with
    Ast.cname_span = dspan;
    ctraits = List.map erase_trait c.Ast.ctraits;
  }

let erase_variant (v : Ast.union_variant) =
  {
    v with
    Ast.vname_span = dspan;
    vpayload = Option.map erase_ty v.Ast.vpayload;
    vtraits = List.map erase_trait v.Ast.vtraits;
  }

let rec erase_kind = function
  | Ast.DStruct { params; members; ops } ->
      Ast.DStruct
        {
          params;
          members = List.map erase_member members;
          ops = List.map erase_decl ops;
        }
  | Ast.DEnum { cases } -> Ast.DEnum { cases = List.map erase_case cases }
  | Ast.DUnion { params; variants } ->
      Ast.DUnion { params; variants = List.map erase_variant variants }
  | Ast.DOp { input; output } ->
      Ast.DOp
        {
          input = Option.map erase_ty input;
          output = Option.map erase_ty output;
        }
  | Ast.DExt { ekind; esig; eraw; ebindings; econformance; _ } ->
      Ast.DExt
        {
          ekind;
          ekind_span = dspan;
          eraw = Option.map (fun _ -> dspan) eraw;
          esig =
            Option.map
              (fun (s : Ast.ext_sig) ->
                {
                  Ast.esig_in = erase_ty s.esig_in;
                  esig_out = erase_ty s.esig_out;
                })
              esig;
          ebindings =
            List.map
              (fun (b : Ast.ext_binding) -> { b with Ast.lang_span = dspan })
              ebindings;
          econformance;
        }
  (* Test blocks are exercised by their own fmt tests; the generator does not
     produce them. *)
  | Ast.DTest _ as k -> k

and erase_decl (d : Ast.decl) =
  {
    d with
    Ast.dname_span = dspan;
    dtraits = List.map erase_trait d.Ast.dtraits;
    dkind = erase_kind d.Ast.dkind;
  }

let erase_import (i : Ast.import) = { i with Ast.ispan = dspan }

let erase_file (f : Ast.file) =
  {
    Ast.imports = List.map erase_import f.Ast.imports;
    decls = List.map erase_decl f.Ast.decls;
  }

(* ── Generators: small pools keep files well-formed and readable ────────── *)

let gen_lname = G.oneof_list [ "id"; "amount_cents"; "note"; "items"; "kind" ]
let gen_tname = G.oneof_list [ "charge"; "card"; "page"; "bank_account" ]
let gen_prim = G.oneof_list [ "bool"; "string"; "i64"; "u32"; "uuid" ]
let gen_params = G.oneof_list [ []; [ "t" ]; [ "t"; "u" ] ]
let gen_qual = G.oneof_list [ "common"; "core"; "c" ]

(* Only parser-reachable shapes: '?' applies to a whole type (never a list
   element or map value), and generic applications carry at least one argument. *)
let rec gen_base n =
  let leaf =
    G.oneof
      [
        (let+ p = gen_prim in
         Ast.TPrim (p, dspan));
        (let+ nm = gen_tname in
         Ast.TName (nm, [], dspan));
        (let+ q = gen_qual and+ nm = gen_tname in
         Ast.TQName (q, nm, [], dspan));
      ]
  in
  if n <= 0 then leaf
  else
    G.oneof
      [
        leaf;
        (let+ nm = gen_tname
         and+ args = G.list_size (G.int_range 1 2) (gen_ty (n - 1)) in
         Ast.TName (nm, args, dspan));
        (let+ q = gen_qual
         and+ nm = gen_tname
         and+ args = G.list_size (G.int_range 1 2) (gen_ty (n - 1)) in
         Ast.TQName (q, nm, args, dspan));
        (let+ e = gen_base (n - 1) in
         Ast.TList (e, dspan));
        (let+ k = gen_ty (n - 1) and+ v = gen_base (n - 1) in
         Ast.TMap (k, v, dspan));
      ]

and gen_ty n =
  G.oneof
    [
      gen_base n;
      (let+ b = gen_base n in
       Ast.TNullable (b, dspan));
    ]

let gen_ty = gen_ty 2

let gen_ref =
  let+ segs = G.list_size (G.int_range 1 3) gen_lname in
  { Ast.segs; ref_span = dspan }

let gen_string =
  G.oneof_list
    [
      "plain";
      "with \"quotes\"";
      "line\nbreak";
      "tab\tand \\";
      "";
      (* templates are ordinary strings to the printer, but they are the shape
         the entry model actually writes *)
      "ENDPOINT_{.client_key}_V2";
      "/notes/{id}";
    ]

let gen_scalar =
  G.oneof
    [
      (let+ s = gen_string in
       Ast.AString s);
      (let+ n = G.oneof_list [ 0; 1; -1; 200; 1000000 ] in
       Ast.AInt n);
      (let+ f = G.oneof_list [ 0.5; -3.75; 100.25; 0.001; 1e10 ] in
       Ast.AFloat f);
      (let+ n = gen_lname in
       Ast.AName n);
      (let+ r = gen_ref in
       Ast.ARef r);
    ]

let gen_arg =
  G.oneof
    [
      gen_scalar;
      (let+ k = gen_lname and+ v = gen_scalar in
       Ast.AKv (k, v));
    ]

let gen_trait =
  let+ name =
    G.oneof_list
      [
        "doc";
        "range";
        "http";
        "errors";
        "deprecated";
        (* the entry model: value sources, derivation, composition, and the
           builtin catalogs, whose "::" lives inside the trait name *)
        "arg";
        "with";
        "env";
        "default";
        "format";
        "bind";
        "header";
        "str::trim";
        "str::upper_snake";
      ]
  and+ args = G.list_size (G.int_range 0 3) gen_arg in
  { Ast.tname = name; targs = args; tspan = dspan }

let gen_traits = G.list_size (G.int_range 0 2) gen_trait

let gen_pattern =
  G.oneof
    [
      (let+ s = gen_string in
       Ast.PString s);
      (let+ n = G.oneof_list [ 0; 1; -1; 200 ] in
       Ast.PInt n);
      (let+ n = gen_lname in
       Ast.PName n);
      G.return Ast.PWildcard;
    ]

let gen_arm_value =
  G.oneof
    [
      (let+ r = gen_ref in
       Ast.AVRef r);
      (let+ s = gen_string in
       Ast.AVString s);
      (let+ n = G.oneof_list [ 0; 1; -1; 200 ] in
       Ast.AVInt n);
      (let+ n = gen_lname in
       Ast.AVName n);
      (* a stack of sources resolved in place: never empty, or the arm would
         print with no value at all *)
      (let+ ts = G.list_size (G.int_range 1 2) gen_trait in
       Ast.AVSources ts);
    ]

let gen_match =
  let+ subject = gen_ref
  and+ arms =
    G.list_size (G.int_range 0 3)
      (let+ pat = gen_pattern and+ value = gen_arm_value in
       { Ast.pat; pat_span = dspan; value; value_span = dspan })
  in
  { Ast.subject; arms; match_span = dspan }

let gen_member =
  let+ name = gen_lname
  and+ ty = gen_ty
  and+ fm = G.oneof [ G.return None; G.map Option.some gen_match ]
  and+ traits = gen_traits in
  {
    Ast.mname = name;
    mname_span = dspan;
    mtype = ty;
    mmatch = fm;
    mtraits = traits;
  }

let gen_case =
  let+ name = gen_lname
  and+ cint = G.oneof [ G.return None; G.map Option.some (G.int_range 0 500) ]
  and+ traits = gen_traits in
  { Ast.cname = name; cname_span = dspan; cint; ctraits = traits }

let gen_variant =
  let+ name = gen_lname
  and+ payload = G.oneof [ G.return None; G.map Option.some gen_ty ]
  and+ traits = gen_traits in
  { Ast.vname = name; vname_span = dspan; vpayload = payload; vtraits = traits }

let gen_opt_ty = G.oneof [ G.return None; G.map Option.some gen_ty ]

(* An op in an entry body takes neither "pub" nor traits above it: the grammar
   gives it only the signature and its trailing traits. *)
let gen_entry_op =
  let+ name = gen_lname
  and+ input = gen_opt_ty
  and+ output = gen_opt_ty
  and+ traits = gen_traits in
  {
    Ast.dname = name;
    dname_span = dspan;
    pub = false;
    dtraits = traits;
    dkind = Ast.DOp { input; output };
  }

let gen_ext_kind =
  G.oneof_list [ Ast.EHook; Ast.EContract; Ast.EConstraint; Ast.EImpl ]

(* "conformance" is a reserved key hoisted out of the bindings, so it is never
   generated as a language tag. *)
let gen_binding =
  let+ lang = G.oneof_list [ "ts"; "go"; "rust"; "python"; "java" ]
  and+ target = G.oneof_list [ "ext/ts/a.ts#f"; "ext\\go\\a \"b\".go#F"; "" ] in
  { Ast.lang; lang_span = dspan; target }

let gen_kind =
  G.oneof
    [
      (let+ params = gen_params
       and+ members = G.list_size (G.int_range 0 4) gen_member
       and+ ops = G.list_size (G.int_range 0 2) gen_entry_op in
       Ast.DStruct { params; members; ops });
      (let+ cases = G.list_size (G.int_range 0 3) gen_case in
       Ast.DEnum { cases });
      (let+ params = gen_params
       and+ variants = G.list_size (G.int_range 0 3) gen_variant in
       Ast.DUnion { params; variants });
      (let+ input = gen_opt_ty and+ output = gen_opt_ty in
       Ast.DOp { input; output });
      (let+ ekind = gen_ext_kind
       and+ raw = G.bool
       and+ esig =
         G.oneof
           [
             G.return None;
             (let+ i = gen_ty and+ o = gen_ty in
              Some { Ast.esig_in = i; esig_out = o });
           ]
       and+ ebindings = G.list_size (G.int_range 0 3) gen_binding
       and+ econformance =
         G.oneof
           [ G.return None; G.map Option.some (G.return "vectors/c.json") ]
       in
       Ast.DExt
         {
           ekind;
           ekind_span = dspan;
           esig;
           eraw = (if raw then Some dspan else None);
           ebindings;
           econformance;
         });
    ]

let gen_decl =
  let+ name = gen_tname
  and+ pub = G.bool
  and+ traits = gen_traits
  and+ kind = gen_kind in
  { Ast.dname = name; dname_span = dspan; pub; dtraits = traits; dkind = kind }

(* Whitespace is not significant, so a trait between an op and the next
   declaration always binds to the op; such a file is not expressible. Strip
   leading traits from any non-op declaration that follows an op so every
   generated file is one the grammar can round-trip. *)
let fix_adjacency (ds : Ast.decl list) : Ast.decl list =
  let rec go = function
    | ({ Ast.dkind = Ast.DOp _; _ } as a) :: b :: rest ->
        let b' =
          match b.Ast.dkind with
          | Ast.DOp _ -> b
          | _ -> { b with Ast.dtraits = [] }
        in
        a :: go (b' :: rest)
    | d :: rest -> d :: go rest
    | [] -> []
  in
  go ds

let gen_import =
  let+ path =
    G.list_size (G.int_range 1 2)
      (G.oneof_list [ "payments"; "common"; "core" ])
  and+ alias =
    G.oneof [ G.return None; G.map Option.some (G.oneof_list [ "c"; "p" ]) ]
  in
  { Ast.imported_path = path; alias; ispan = dspan }

let gen_file =
  let+ imports = G.list_size (G.int_range 0 2) gen_import
  and+ ds = G.list_size (G.int_range 0 5) gen_decl in
  { Ast.imports; decls = fix_adjacency ds }

let roundtrip =
  QCheck.Test.make ~count:500 ~name:"parse (print ast) = ast, spans aside"
    (QCheck.make ~print:Printer.print_file gen_file) (fun file ->
      let printed = Printer.print_file file in
      let reparsed, diags = Parser.parse printed in
      diags = []
      && erase_file reparsed = erase_file file
      && String.equal (Printer.print_file reparsed) printed)

let () = QCheck_base_runner.run_tests_main [ roundtrip ]
