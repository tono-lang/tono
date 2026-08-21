(* Span erasure for the fmt round-trip property: the parsed and the
   generated ASTs are compared structurally once every span is reset to a
   dummy, so only the syntax counts. Split out of [fmt_property_test.ml] to
   stay under the file-size cap; the property itself and its generators live
   there. *)

open Tono_frontend

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

let rec erase_ref (r : Ast.ref_path) =
  { r with Ast.ref_span = dspan; index = Option.map erase_ref r.Ast.index }

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
  | Ast.CaLit (l, _) -> Ast.CaLit (l, dspan)
  | Ast.CaCall nc -> Ast.CaCall (erase_nested_call nc)
  | Ast.CaList (items, _) -> Ast.CaList (List.map erase_call_arg items, dspan)
  | Ast.CaType (n, _) -> Ast.CaType (n, dspan)
  | Ast.CaMap (entries, _) ->
      Ast.CaMap
        (List.map (fun (k, _, v) -> (k, dspan, erase_call_arg v)) entries, dspan)

and erase_nested_call (nc : Ast.nested_call) : Ast.nested_call =
  {
    nc with
    Ast.nc_symbol_span = dspan;
    nc_args = List.map erase_call_arg nc.Ast.nc_args;
    nc_span = dspan;
  }

and erase_call_expr (ce : Ast.call_expr) : Ast.call_expr =
  {
    ce with
    Ast.ce_head_span = dspan;
    ce_args = List.map erase_call_arg ce.Ast.ce_args;
    ce_span = dspan;
  }

let erase_trait (t : Ast.trait) =
  { t with Ast.tspan = dspan; targs = List.map erase_arg t.Ast.targs }

let erase_op_impl (oi : Ast.op_impl) : Ast.op_impl =
  {
    Ast.oi_recv = erase_ref oi.Ast.oi_recv;
    oi_method = oi.Ast.oi_method;
    oi_method_span = dspan;
    oi_args = List.map erase_call_arg oi.Ast.oi_args;
    oi_span = dspan;
  }

let erase_arm_value = function
  | Ast.AVRef r -> Ast.AVRef (erase_ref r)
  | Ast.AVSources ts -> Ast.AVSources (List.map erase_trait ts)
  | Ast.AVSubject _ -> Ast.AVSubject dspan
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
  | Ast.MHandleCall hc -> Ast.MHandleCall (erase_op_impl hc)

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
    elb_call_receiver_span =
      Option.map (fun _ -> dspan) b.Ast.elb_call_receiver_span;
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

let erase_opaque_names = function
  | Ast.OnShared (name, _) -> Ast.OnShared (name, dspan)
  | Ast.OnPerLang entries ->
      Ast.OnPerLang
        (List.map
           (fun (e : Ast.opaque_name_entry) ->
             { e with Ast.one_lang_span = dspan; one_name_span = dspan })
           entries)

let erase_opaque_instance (i : Ast.opaque_instance) =
  {
    Ast.oi_names = erase_opaque_names i.Ast.oi_names;
    oi_arg = erase_ty i.Ast.oi_arg;
    oi_arg_span = dspan;
    oi_span = dspan;
  }

let erase_opaque_type (t : Ast.opaque_type) =
  {
    t with
    Ast.opq_name_span = dspan;
    opq_instance = Option.map erase_opaque_instance t.Ast.opq_instance;
    opq_methods = List.map erase_extern_decl t.Ast.opq_methods;
    opq_span = dspan;
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
  | Ast.DOp { pname; input; output; oimpl } ->
      Ast.DOp
        {
          pname;
          input = Option.map erase_ty input;
          output = Option.map erase_ty output;
          oimpl = Option.map erase_op_impl oimpl;
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
