(* Lowering for the FFI ext surface: an extern call argument (shared by a
   field's own [= ns.fn(args)] source and an ext block's own [call:] line),
   and the [ext <name> { ... }] library block itself. Split out of [Lower]
   to keep that file under the line-count cap. Structural lowering only: no
   resolution of an op's arity/types against its callers or of a [returns:]
   field ref against its [yields:] name -- that is [Check_ext_lib]'s job at
   the AST stage. [lower_type]/[lower_select] are threaded in (as
   [Parser_ext] threads [parse_type]) to avoid a dependency cycle with
   [Lower]. *)

(* An extern call argument: resolving [ns]/[fn] against a declared [ext]
   block is deferred (out of scope); this only carries the call structured.
   A ctor field's own value is the general trait-value grammar (it can be a
   literal, list, or nested call, e.g. [opts { retries: 3 }]), so
   [lower_ctor_field_value] converts that richer shape down to [Ir.call_arg].
   [handles] are the opaque handles of the enclosing ext block: a bare name
   that names one (and no logical parameter) is a class reference, the
   handle itself passed for a library that constructs on its own. Outside a
   [call:] line there is no block in scope, so the list is empty. *)
let rec lower_call_arg ?(handles = []) ?(params = []) :
    Ast.call_arg -> Ir.call_arg = function
  | Ast.CaParam (n, _) ->
      if List.mem n handles && not (List.mem n params) then Ir.Ca_type n
      else Ir.Ca_param n
  | Ast.CaParamAs (n, _, sp, _) -> Ir.Ca_param_as (n, sp)
  | Ast.CaRef r -> Ir.Ca_ref r.segs
  | Ast.CaCtor c -> Ir.Ca_ctor (lower_call_ctor c)
  | Ast.CaCtorAs (c, sp, _) ->
      Ir.Ca_ctor { (lower_call_ctor c) with Ir.cc_as = Some sp }
  | Ast.CaLit (Ast.LStr s, _) -> Ir.Ca_lit (`String s)
  | Ast.CaLit (Ast.LInt n, _) -> Ir.Ca_lit (`Int n)
  | Ast.CaLit (Ast.LFloat f, _) -> Ir.Ca_lit (`Float f)
  | Ast.CaCall nc -> Ir.Ca_symbol_call (lower_nested_call ~handles ~params nc)
  | Ast.CaList (items, _) ->
      Ir.Ca_list (List.map (lower_call_arg ~handles ~params) items)
  | Ast.CaForeign (s, _) -> Ir.Ca_foreign s

and lower_nested_call ~handles ~params (nc : Ast.nested_call) : Ir.symbol_call =
  {
    Ir.scl_symbol = nc.nc_symbol;
    scl_args = List.map (lower_call_arg ~handles ~params) nc.nc_args;
  }

and lower_call_ctor (c : Ast.ctor_arg) : Ir.call_ctor =
  {
    Ir.cc_name = c.ctor_name;
    cc_fields =
      List.map (fun (n, _, v) -> (n, lower_ctor_field_value v)) c.ctor_fields;
    cc_as = None;
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
    ec_args = List.map (fun a -> lower_call_arg a) ce.ce_args;
  }

let lower_lang_path (lp : Ast.lang_path) : Ir.lang_path =
  { Ir.lgp_lang = lp.lp_lang; lgp_path = lp.lp_path }

let lower_lang_block (b : Ast.lang_block) : Ir.foreign_lang =
  {
    Ir.fl_lang = b.lb_lang;
    fl_head = b.lb_head;
    fl_fields = List.map (fun (n, _, sp, _) -> (n, sp)) b.lb_fields;
  }

(* The language blocks of a top-level struct (an error struct), carried in
   the shape's trait bag under "foreign", the way @doc travels: the shape
   record stays unchanged and a reader that does not know the key ignores
   it. Absent when the struct has no block. *)
let foreign_trait (langs : Ast.lang_block list) : Ir.trait list =
  match langs with
  | [] -> []
  | _ ->
      [
        {
          Ir.trait_id = "foreign";
          value =
            `List
              (List.map
                 (fun b ->
                   Ir_json_extern.encode_foreign_lang (lower_lang_block b))
                 langs);
        };
      ]

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
    fgs_langs = List.map lower_lang_block s.fs_langs;
  }

let lower_yields_pos ~lower_type ~resolve ~diags (y : Ast.yields_pos) :
    Ir.yields_pos =
  let base =
    {
      Ir.yp_name = y.yp_name;
      yp_type = None;
      yp_is_error = false;
      yp_foreign = None;
    }
  in
  match y.yp_ty with
  | Ast.YType t ->
      { base with yp_type = Some (lower_type ~params:[] ~resolve ~diags t) }
  | Ast.YError _ -> { base with yp_is_error = true }
  | Ast.YForeign (s, _) -> { base with yp_foreign = Some s }

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

let lower_extern_lang_body ~lower_type ~lower_select ~resolve ~diags ~handles
    ~params (b : Ast.extern_lang_body) : Ir.extern_lang =
  {
    Ir.el_lang = b.elb_lang;
    el_symbol = b.elb_call_symbol;
    el_call_args = List.map (lower_call_arg ~handles ~params) b.elb_call_args;
    el_chain = Option.map (lower_nested_call ~handles ~params) b.elb_call_chain;
    el_yields =
      (match b.elb_yields with
      | None -> []
      | Some ys -> List.map (lower_yields_pos ~lower_type ~resolve ~diags) ys);
    el_returns =
      Option.map
        (lower_returns ~lower_type ~lower_select ~resolve ~diags)
        b.elb_returns;
  }

(* The names an op's trait lists: @async(rust, ts), @errors(a, b). Repeats
   collapse in declaration order. *)
let trait_names (name : string) (traits : Ast.trait list) : string list =
  let names =
    List.concat_map
      (fun (t : Ast.trait) ->
        if String.equal t.tname name then
          List.filter_map (function Ast.AName n -> Some n | _ -> None) t.targs
        else [])
      traits
  in
  List.fold_left
    (fun acc n -> if List.mem n acc then acc else acc @ [ n ])
    [] names

let lower_extern ~lower_type ~lower_select ~resolve ~diags ~handles
    (e : Ast.extern_decl) : Ir.extern_decl =
  let params = List.map (fun (p : Ast.extern_param) -> p.ep_name) e.ed_params in
  {
    Ir.x_name = e.ed_name;
    x_params =
      List.map
        (fun (p : Ast.extern_param) ->
          {
            Ir.xp_name = p.ep_name;
            xp_type = lower_type ~params:[] ~resolve ~diags p.ep_type;
          })
        e.ed_params;
    x_return = lower_type ~params:[] ~resolve ~diags e.ed_return;
    x_langs =
      List.map
        (lower_extern_lang_body ~lower_type ~lower_select ~resolve ~diags
           ~handles ~params)
        e.ed_langs;
    x_async = trait_names "async" e.ed_traits;
    x_errors =
      List.map
        (fun n -> resolve ~qualifier:None ~name:n)
        (trait_names "errors" e.ed_traits);
  }

let lower_opaque_type ~lower_type ~lower_select ~resolve ~diags ~handles
    (t : Ast.opaque_type) : Ir.opaque_type =
  {
    Ir.opq_name = t.opq_name;
    opq_langs = List.map lower_lang_block t.opq_langs;
    opq_methods =
      List.map
        (lower_extern ~lower_type ~lower_select ~resolve ~diags ~handles)
        t.opq_methods;
  }

let lower_ext_lib ~lower_type ~lower_select ~resolve ~diags (d : Ast.decl) :
    Ir.ext_lib =
  match d.dkind with
  | Ast.DExtLib { body; _ } ->
      let handles =
        List.map (fun (t : Ast.opaque_type) -> t.opq_name) body.elib_types
      in
      {
        Ir.xl_name = d.dname;
        xl_langs = List.map lower_lang_path body.elib_langs;
        xl_structs =
          List.map
            (lower_foreign_struct ~lower_type ~resolve ~diags)
            body.elib_structs;
        xl_types =
          List.map
            (lower_opaque_type ~lower_type ~lower_select ~resolve ~diags
               ~handles)
            body.elib_types;
        xl_externs =
          List.map
            (lower_extern ~lower_type ~lower_select ~resolve ~diags ~handles)
            body.elib_externs;
      }
  | _ -> assert false
