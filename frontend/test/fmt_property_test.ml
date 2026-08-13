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

(* A trait argument can carry a reference or a call expression, both of which
   carry spans of their own; a call argument's own ctor field values are the
   same trait-argument grammar, so the two groups are mutually recursive. *)
let rec erase_arg = function
  | Ast.ARef r -> Ast.ARef (erase_ref r)
  | Ast.AKv (k, v) -> Ast.AKv (k, erase_arg v)
  | Ast.AList xs -> Ast.AList (List.map erase_arg xs)
  | Ast.ACtor c -> Ast.ACtor (erase_ctor c)
  | Ast.ACall ce -> Ast.ACall (erase_call_expr ce)
  | (Ast.AString _ | Ast.AInt _ | Ast.AFloat _ | Ast.AName _) as a -> a

and erase_ctor (c : Ast.ctor_arg) : Ast.ctor_arg =
  {
    c with
    Ast.ctor_name_span = dspan;
    ctor_span = dspan;
    ctor_fields =
      List.map (fun (n, _, v) -> (n, dspan, erase_arg v)) c.Ast.ctor_fields;
  }

and erase_call_arg = function
  | Ast.CaParam (n, _) -> Ast.CaParam (n, dspan)
  | Ast.CaRef r -> Ast.CaRef (erase_ref r)
  | Ast.CaCtor c -> Ast.CaCtor (erase_ctor c)

and erase_call_expr (ce : Ast.call_expr) : Ast.call_expr =
  {
    ce with
    Ast.ce_head_span = dspan;
    ce_args = List.map erase_call_arg ce.Ast.ce_args;
    ce_span = dspan;
  }

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

let erase_member_value = function
  | Ast.MMatch fm -> Ast.MMatch (erase_match fm)
  | Ast.MCall ce -> Ast.MCall (erase_call_expr ce)

let erase_member (m : Ast.member) =
  {
    m with
    Ast.mname_span = dspan;
    mtype = erase_ty m.Ast.mtype;
    mvalue = Option.map erase_member_value m.Ast.mvalue;
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

(* ── FFI library blocks: ext <name> { ... } ──────────────────────────────── *)

let erase_lang_path (lp : Ast.lang_path) = { lp with Ast.lp_lang_span = dspan }

let erase_foreign_field (f : Ast.foreign_field) =
  { f with Ast.ff_name_span = dspan; ff_type = erase_ty f.Ast.ff_type }

let erase_foreign_struct (s : Ast.foreign_struct) =
  {
    s with
    Ast.fs_name_span = dspan;
    fs_fields = List.map erase_foreign_field s.Ast.fs_fields;
    fs_span = dspan;
  }

let erase_yields_ty = function
  | Ast.YType t -> Ast.YType (erase_ty t)
  | Ast.YError _ -> Ast.YError dspan

let erase_yields_pos (y : Ast.yields_pos) =
  { y with Ast.yp_name_span = dspan; yp_ty = erase_yields_ty y.Ast.yp_ty }

let erase_returns_value = function
  | Ast.RvRef r -> Ast.RvRef (erase_ref r)
  | Ast.RvMatch fm -> Ast.RvMatch (erase_match fm)

let erase_returns_field (f : Ast.returns_field) =
  {
    f with
    Ast.rf_name_span = dspan;
    rf_value = erase_returns_value f.Ast.rf_value;
    rf_span = dspan;
  }

let erase_returns_lit (r : Ast.returns_lit) =
  {
    Ast.rl_type = erase_ty r.Ast.rl_type;
    rl_fields = List.map erase_returns_field r.Ast.rl_fields;
    rl_span = dspan;
  }

let erase_error_map_entry (e : Ast.error_map_entry) =
  { e with Ast.em_sentinel_span = dspan; em_type_span = dspan }

let erase_extern_lang_body (b : Ast.extern_lang_body) =
  {
    b with
    Ast.elb_lang_span = dspan;
    elb_call_symbol_span = dspan;
    elb_call_args = List.map erase_call_arg b.Ast.elb_call_args;
    elb_yields = Option.map (List.map erase_yields_pos) b.Ast.elb_yields;
    elb_returns = Option.map erase_returns_lit b.Ast.elb_returns;
    elb_errors = List.map erase_error_map_entry b.Ast.elb_errors;
    elb_span = dspan;
  }

let erase_extern_param (p : Ast.extern_param) =
  { p with Ast.ep_name_span = dspan; ep_type = erase_ty p.Ast.ep_type }

let erase_extern_decl (e : Ast.extern_decl) =
  {
    e with
    Ast.ed_name_span = dspan;
    ed_params = List.map erase_extern_param e.Ast.ed_params;
    ed_return = erase_ty e.Ast.ed_return;
    ed_langs = List.map erase_extern_lang_body e.Ast.ed_langs;
    ed_span = dspan;
  }

let erase_opaque_type (t : Ast.opaque_type) =
  {
    t with
    Ast.ot_name_span = dspan;
    ot_methods = List.map erase_extern_decl t.Ast.ot_methods;
    ot_span = dspan;
  }

let erase_ext_lib_body (b : Ast.ext_lib_body) : Ast.ext_lib_body =
  {
    Ast.elib_langs = List.map erase_lang_path b.Ast.elib_langs;
    elib_structs = List.map erase_foreign_struct b.Ast.elib_structs;
    elib_types = List.map erase_opaque_type b.Ast.elib_types;
    elib_externs = List.map erase_extern_decl b.Ast.elib_externs;
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
  | Ast.DOp { pname; input; output } ->
      Ast.DOp
        {
          pname;
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
  | Ast.DExtLib { body; span = _ } ->
      Ast.DExtLib { body = erase_ext_lib_body body; span = dspan }
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
      (let+ k = gen_lname
       and+ ns = G.list_size (G.int_range 1 3) (G.int_range 0 999) in
       Ast.AKv (k, Ast.AList (List.map (fun n -> Ast.AInt n) ns)));
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

(* ── FFI call expressions: ns.fn(args) ───────────────────────────────────── *)

let gen_ctor_arg =
  let+ name = gen_tname
  and+ fields =
    G.list_size (G.int_range 0 2)
      (let+ fname = gen_lname and+ v = gen_scalar in
       (fname, dspan, v))
  in
  { Ast.ctor_name = name; ctor_name_span = dspan; ctor_fields = fields; ctor_span = dspan }

let gen_call_arg =
  G.oneof
    [
      (let+ n = gen_lname in
       Ast.CaParam (n, dspan));
      (let+ r = gen_ref in
       Ast.CaRef r);
      (let+ c = gen_ctor_arg in
       Ast.CaCtor c);
    ]

let gen_call_expr =
  let+ ns = gen_tname
  and+ fn = gen_lname
  and+ args = G.list_size (G.int_range 0 2) gen_call_arg in
  { Ast.ce_ns = ns; ce_fn = fn; ce_head_span = dspan; ce_args = args; ce_span = dspan }

let gen_member_value =
  G.oneof
    [
      G.return None;
      (let+ fm = gen_match in
       Some (Ast.MMatch fm));
      (let+ ce = gen_call_expr in
       Some (Ast.MCall ce));
    ]

let gen_member =
  let+ name = gen_lname
  and+ ty = gen_ty
  and+ mv = gen_member_value
  and+ traits = gen_traits in
  {
    Ast.mname = name;
    mname_span = dspan;
    mtype = ty;
    mvalue = mv;
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
   gives it only the signature and its trailing traits. A present input always
   carries a name; only a parameterless op has no name. *)
let gen_pname = gen_lname

let gen_entry_op =
  let+ name = gen_lname
  and+ pname = gen_pname
  and+ input = gen_opt_ty
  and+ output = gen_opt_ty
  and+ traits = gen_traits in
  {
    Ast.dname = name;
    dname_span = dspan;
    pub = false;
    dtraits = traits;
    dkind =
      Ast.DOp
        { pname = (if input = None then None else Some pname); input; output };
  }

let gen_ext_kind =
  G.oneof_list [ Ast.EHook; Ast.EContract; Ast.EConstraint; Ast.EImpl ]

(* "conformance" is a reserved key hoisted out of the bindings, so it is never
   generated as a language tag. *)
let gen_binding =
  let+ lang = G.oneof_list [ "ts"; "go"; "rust"; "python"; "java" ]
  and+ target = G.oneof_list [ "ext/ts/a.ts#f"; "ext\\go\\a \"b\".go#F"; "" ] in
  { Ast.lang; lang_span = dspan; target }

(* ── FFI library blocks: ext <name> { ... } ──────────────────────────────── *)

let gen_lang_path =
  let+ lang = G.oneof_list [ "go"; "ts"; "rust" ] and+ path = gen_string in
  { Ast.lp_lang = lang; lp_lang_span = dspan; lp_path = path }

let gen_foreign_field =
  let+ name = gen_tname and+ ty = gen_ty in
  { Ast.ff_name = name; ff_name_span = dspan; ff_type = ty }

let gen_foreign_struct =
  let+ name = gen_tname
  and+ fields = G.list_size (G.int_range 0 2) gen_foreign_field in
  { Ast.fs_name = name; fs_name_span = dspan; fs_fields = fields; fs_span = dspan }

let gen_yields_ty =
  G.oneof [ (let+ t = gen_ty in Ast.YType t); G.return (Ast.YError dspan) ]

let gen_yields_pos =
  let+ name = gen_lname and+ ty = gen_yields_ty in
  { Ast.yp_name = name; yp_name_span = dspan; yp_ty = ty }

let gen_returns_value =
  G.oneof
    [ (let+ r = gen_ref in Ast.RvRef r); (let+ fm = gen_match in Ast.RvMatch fm) ]

let gen_returns_field =
  let+ name = gen_lname and+ v = gen_returns_value in
  { Ast.rf_name = name; rf_name_span = dspan; rf_value = v; rf_span = dspan }

let gen_returns_lit =
  let+ ty = gen_ty
  and+ fields = G.list_size (G.int_range 1 2) gen_returns_field in
  { Ast.rl_type = ty; rl_fields = fields; rl_span = dspan }

let gen_error_map_entry =
  let+ sentinel = gen_string and+ ty = gen_tname in
  {
    Ast.em_sentinel = sentinel;
    em_sentinel_span = dspan;
    em_type = ty;
    em_type_span = dspan;
  }

let gen_extern_lang_body =
  let+ lang = G.oneof_list [ "go"; "ts"; "rust" ]
  and+ symbol = gen_string
  and+ args = G.list_size (G.int_range 0 2) gen_call_arg
  and+ yields =
    G.oneof
      [
        G.return None;
        (let+ ys = G.list_size (G.int_range 1 2) gen_yields_pos in
         Some ys);
      ]
  and+ returns =
    G.oneof [ G.return None; (let+ r = gen_returns_lit in Some r) ]
  and+ errors = G.list_size (G.int_range 0 2) gen_error_map_entry in
  {
    Ast.elb_lang = lang;
    elb_lang_span = dspan;
    elb_call_symbol = symbol;
    elb_call_symbol_span = dspan;
    elb_call_args = args;
    elb_yields = yields;
    elb_returns = returns;
    elb_errors = errors;
    elb_span = dspan;
  }

let gen_extern_param =
  let+ name = gen_lname and+ ty = gen_ty in
  { Ast.ep_name = name; ep_name_span = dspan; ep_type = ty }

let gen_extern_decl =
  let+ name = gen_lname
  and+ params = G.list_size (G.int_range 0 2) gen_extern_param
  and+ ret = gen_ty
  and+ langs = G.list_size (G.int_range 1 2) gen_extern_lang_body in
  {
    Ast.ed_name = name;
    ed_name_span = dspan;
    ed_params = params;
    ed_return = ret;
    ed_langs = langs;
    ed_span = dspan;
  }

let gen_opaque_type =
  let+ name = gen_tname
  and+ methods = G.list_size (G.int_range 0 1) gen_extern_decl in
  { Ast.ot_name = name; ot_name_span = dspan; ot_methods = methods; ot_span = dspan }

let gen_ext_lib_body =
  let+ langs = G.list_size (G.int_range 0 2) gen_lang_path
  and+ structs = G.list_size (G.int_range 0 2) gen_foreign_struct
  and+ types = G.list_size (G.int_range 0 1) gen_opaque_type
  and+ externs = G.list_size (G.int_range 0 2) gen_extern_decl in
  {
    Ast.elib_langs = langs;
    elib_structs = structs;
    elib_types = types;
    elib_externs = externs;
  }

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
      (let+ pname = gen_pname
       and+ input = gen_opt_ty
       and+ output = gen_opt_ty in
       Ast.DOp
         { pname = (if input = None then None else Some pname); input; output });
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
      (let+ body = gen_ext_lib_body in
       Ast.DExtLib { body; span = dspan });
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
