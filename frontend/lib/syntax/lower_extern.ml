(* Lowering for the FFI ext/extern surface: an extern call argument (shared
   by a field's own [= ns.fn(args)] source and an ext block's own [call:]
   line), and the [ext <name> { ... }] library block itself. Split out of
   [Lower] to keep that file under the line-count cap. Structural lowering
   only: no resolution of an extern's arity/types against its callers, of an
   [errors:] sentinel against a declared error shape, or of a [returns:]
   field ref against its [yields:] name -- that is a later pass (see
   [Ir.ext_lib]'s doc comment). [lower_type]/[lower_select] are threaded in
   (as [Parser_ext] threads [parse_type]) to avoid a dependency cycle with
   [Lower]. *)

(* An extern call argument: resolving [ns]/[fn] against a declared [ext]
   block is deferred (out of scope); this only carries the call structured.
   A ctor field's own value is the general trait-value grammar (it can be a
   literal, list, or nested call, e.g. [opts { retries: 3 }]), so
   [lower_ctor_field_value] converts that richer shape down to [Ir.call_arg]. *)
let rec lower_call_arg : Ast.call_arg -> Ir.call_arg = function
  | Ast.CaParam (n, _) -> Ir.Ca_param n
  | Ast.CaRef r -> Ir.Ca_ref r.segs
  | Ast.CaCtor c -> Ir.Ca_ctor (lower_call_ctor c)
  | Ast.CaLit (Ast.LStr s, _) -> Ir.Ca_lit (`String s)
  | Ast.CaLit (Ast.LInt n, _) -> Ir.Ca_lit (`Int n)
  | Ast.CaLit (Ast.LFloat f, _) -> Ir.Ca_lit (`Float f)
  | Ast.CaCall nc ->
      Ir.Ca_symbol_call
        {
          Ir.scl_symbol = nc.nc_symbol;
          scl_args = List.map lower_call_arg nc.nc_args;
        }
  | Ast.CaList (items, _) -> Ir.Ca_list (List.map lower_call_arg items)

and lower_call_ctor (c : Ast.ctor_arg) : Ir.call_ctor =
  {
    Ir.cc_name = c.ctor_name;
    cc_fields =
      List.map (fun (n, _, v) -> (n, lower_ctor_field_value v)) c.ctor_fields;
  }

and lower_ctor_field_value : Ast.trait_arg -> Ir.call_arg = function
  | Ast.AString s -> Ir.Ca_lit (`String s)
  | Ast.AInt n -> Ir.Ca_lit (`Int n)
  | Ast.AFloat f -> Ir.Ca_lit (`Float f)
  | Ast.AName n -> Ir.Ca_param n
  | Ast.ARef r -> Ir.Ca_ref r.segs
  | Ast.AKv (_, v) -> lower_ctor_field_value v
  | Ast.AList xs -> Ir.Ca_list (List.map lower_ctor_field_value xs)
  | Ast.ACtor c -> Ir.Ca_ctor (lower_call_ctor c)
  | Ast.ACall ce -> Ir.Ca_call (lower_call_expr ce)

and lower_call_expr (ce : Ast.call_expr) : Ir.entry_call =
  {
    Ir.ec_ns = ce.ce_ns;
    ec_fn = ce.ce_fn;
    ec_args = List.map lower_call_arg ce.ce_args;
  }

let lower_lang_path (lp : Ast.lang_path) : Ir.lang_path =
  { Ir.lgp_lang = lp.lp_lang; lgp_path = lp.lp_path }

let lower_foreign_field ~lower_type ~resolve ~diags (f : Ast.foreign_field) :
    Ir.foreign_field =
  {
    Ir.fgf_name = f.ff_name;
    fgf_type = lower_type ~params:[] ~resolve ~diags f.ff_type;
  }

let lower_foreign_struct ~lower_type ~resolve ~diags (s : Ast.foreign_struct) :
    Ir.foreign_struct =
  {
    Ir.fgs_name = s.fs_name;
    fgs_fields =
      List.map (lower_foreign_field ~lower_type ~resolve ~diags) s.fs_fields;
  }

let lower_yields_pos ~lower_type ~resolve ~diags (y : Ast.yields_pos) :
    Ir.yields_pos =
  match y.yp_ty with
  | Ast.YType t ->
      {
        Ir.yp_name = y.yp_name;
        yp_type = Some (lower_type ~params:[] ~resolve ~diags t);
        yp_is_error = false;
      }
  | Ast.YError _ ->
      { Ir.yp_name = y.yp_name; yp_type = None; yp_is_error = true }

let lower_returns_value ~lower_select ~diags :
    Ast.returns_value -> Ir.returns_value = function
  | Ast.RvRef r -> Ir.Rv_ref r.segs
  | Ast.RvMatch fm -> Ir.Rv_select (lower_select ~diags fm)

let lower_returns ~lower_type ~lower_select ~resolve ~diags
    (r : Ast.returns_lit) : Ir.returns_lit =
  {
    Ir.rvl_type = lower_type ~params:[] ~resolve ~diags r.rl_type;
    rvl_fields =
      List.map
        (fun (f : Ast.returns_field) ->
          {
            Ir.rvf_name = f.rf_name;
            rvf_value = lower_returns_value ~lower_select ~diags f.rf_value;
          })
        r.rl_fields;
  }

let lower_error_binding (e : Ast.error_map_entry) : Ir.error_binding =
  { Ir.erb_sentinel = e.em_sentinel; erb_type = e.em_type }

let lower_extern_lang_body ~lower_type ~lower_select ~resolve ~diags
    (b : Ast.extern_lang_body) : Ir.extern_lang =
  {
    Ir.el_lang = b.elb_lang;
    el_symbol = b.elb_call_symbol;
    el_call_args = List.map lower_call_arg b.elb_call_args;
    el_yields =
      (match b.elb_yields with
      | None -> []
      | Some ys -> List.map (lower_yields_pos ~lower_type ~resolve ~diags) ys);
    el_returns =
      Option.map
        (lower_returns ~lower_type ~lower_select ~resolve ~diags)
        b.elb_returns;
    el_errors = List.map lower_error_binding b.elb_errors;
    el_sync = b.elb_sync;
    el_infallible = b.elb_infallible;
    el_ctx = b.elb_ctx;
    el_new = b.elb_new;
  }

let rec lower_extern ~lower_type ~lower_select ~resolve ~diags
    (e : Ast.extern_decl) : Ir.extern_decl =
  {
    Ir.x_name = e.ed_name;
    x_params =
      List.map
        (fun (p : Ast.extern_param) ->
          {
            Ir.xp_name = p.ep_name;
            xp_type = lower_type ~params:[] ~resolve ~diags p.ep_type;
            xp_variadic = p.ep_variadic;
          })
        e.ed_params;
    x_return = lower_type ~params:[] ~resolve ~diags e.ed_return;
    x_langs =
      List.map
        (lower_extern_lang_body ~lower_type ~lower_select ~resolve ~diags)
        e.ed_langs;
  }

and lower_opaque_type ~lower_type ~lower_select ~resolve ~diags ~langs
    (t : Ast.opaque_type) : Ir.opaque_type =
  {
    Ir.opq_name = t.opq_name;
    opq_instance =
      Option.map
        (fun (i : Ast.opaque_instance) ->
          (* A shared surface name is expanded here to one entry per language
             the ext declares a module path for, so IR consumers only ever
             look a language up; the keyed surface form lowers verbatim. *)
          let names =
            match i.Ast.oi_names with
            | Ast.OnShared (name, _) ->
                List.map
                  (fun lang -> { Ir.inn_lang = lang; inn_name = name })
                  langs
            | Ast.OnPerLang entries ->
                List.map
                  (fun (e : Ast.opaque_name_entry) ->
                    { Ir.inn_lang = e.one_lang; inn_name = e.one_name })
                  entries
          in
          {
            Ir.inst_names = names;
            inst_arg = lower_type ~params:[] ~resolve ~diags i.oi_arg;
          })
        t.opq_instance;
    opq_interface = t.opq_interface;
    opq_methods =
      List.map
        (lower_extern ~lower_type ~lower_select ~resolve ~diags)
        t.opq_methods;
  }

let lower_ext_lib ~lower_type ~lower_select ~resolve ~diags (d : Ast.decl) :
    Ir.ext_lib =
  match d.dkind with
  | Ast.DExtLib { body; _ } ->
      {
        Ir.xl_name = d.dname;
        xl_langs = List.map lower_lang_path body.elib_langs;
        xl_structs =
          List.map
            (lower_foreign_struct ~lower_type ~resolve ~diags)
            body.elib_structs;
        xl_types =
          (let langs =
             List.map (fun (lp : Ast.lang_path) -> lp.lp_lang) body.elib_langs
           in
           List.map
             (lower_opaque_type ~lower_type ~lower_select ~resolve ~diags ~langs)
             body.elib_types);
        xl_externs =
          List.map
            (lower_extern ~lower_type ~lower_select ~resolve ~diags)
            body.elib_externs;
      }
  | _ -> assert false
