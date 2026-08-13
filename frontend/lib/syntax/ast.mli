(* Surface AST produced by the parser and consumed by [Lower]. *)

type ty =
  | TPrim of string * Span.span
  | TName of string * ty list * Span.span
  | TQName of string * string * ty list * Span.span
  | TList of ty * Span.span
  | TMap of ty * ty * Span.span
  | TNullable of ty * Span.span
  | TError of Span.span

type ref_path = { segs : string list; ref_span : Span.span }

type trait_arg =
  | AString of string
  | AInt of int
  | AFloat of float
  | AName of string
  | ARef of ref_path
  | AKv of string * trait_arg
  | AList of trait_arg list
  | ACtor of ctor_arg
  | ACall of call_expr

and ctor_arg = {
  ctor_name : string;
  ctor_name_span : Span.span;
  ctor_fields : (string * Span.span * trait_arg) list;
  ctor_span : Span.span;
}

and call_arg =
  | CaParam of string * Span.span
  | CaRef of ref_path
  | CaCtor of ctor_arg

and call_expr = {
  ce_ns : string;
  ce_fn : string;
  ce_head_span : Span.span;
  ce_args : call_arg list;
  ce_span : Span.span;
}

type trait = { tname : string; targs : trait_arg list; tspan : Span.span }

type match_pattern =
  | PString of string
  | PInt of int
  | PName of string
  | PWildcard

type arm_value =
  | AVRef of ref_path
  | AVString of string
  | AVInt of int
  | AVName of string
  | AVSources of trait list

type match_arm = {
  pat : match_pattern;
  pat_span : Span.span;
  value : arm_value;
  value_span : Span.span;
}

type field_match = {
  subject : ref_path;
  arms : match_arm list;
  match_span : Span.span;
}

type member_value = MMatch of field_match | MCall of call_expr

type member = {
  mname : string;
  mname_span : Span.span;
  mtype : ty;
  mvalue : member_value option;
  mtraits : trait list;
}

type enum_case = {
  cname : string;
  cname_span : Span.span;
  cint : int option;
  ctraits : trait list;
}

type union_variant = {
  vname : string;
  vname_span : Span.span;
  vpayload : ty option;
  vtraits : trait list;
}

type ext_kind = EHook | EContract | EConstraint | EImpl
type ext_binding = { lang : string; lang_span : Span.span; target : string }
type ext_sig = { esig_in : ty; esig_out : ty }

(* FFI library blocks: ext <name> { ... } (see ast.ml for shape commentary). *)
type lang_path = {
  lp_lang : string;
  lp_lang_span : Span.span;
  lp_path : string;
}

type foreign_field = {
  ff_name : string;
  ff_name_span : Span.span;
  ff_type : ty;
}

type foreign_struct = {
  fs_name : string;
  fs_name_span : Span.span;
  fs_fields : foreign_field list;
  fs_span : Span.span;
}

type yields_ty = YType of ty | YError of Span.span

type yields_pos = {
  yp_name : string;
  yp_name_span : Span.span;
  yp_ty : yields_ty;
}

type returns_value = RvRef of ref_path | RvMatch of field_match

type returns_field = {
  rf_name : string;
  rf_name_span : Span.span;
  rf_value : returns_value;
  rf_span : Span.span;
}

type returns_lit = {
  rl_type : ty;
  rl_fields : returns_field list;
  rl_span : Span.span;
}

type error_map_entry = {
  em_sentinel : string;
  em_sentinel_span : Span.span;
  em_type : string;
  em_type_span : Span.span;
}

type extern_lang_body = {
  elb_lang : string;
  elb_lang_span : Span.span;
  elb_call_symbol : string;
  elb_call_symbol_span : Span.span;
  elb_call_args : call_arg list;
  elb_yields : yields_pos list option;
  elb_returns : returns_lit option;
  elb_errors : error_map_entry list;
  elb_span : Span.span;
}

type extern_param = { ep_name : string; ep_name_span : Span.span; ep_type : ty }

type extern_decl = {
  ed_name : string;
  ed_name_span : Span.span;
  ed_params : extern_param list;
  ed_return : ty;
  ed_langs : extern_lang_body list;
  ed_span : Span.span;
}

type opaque_type = {
  opq_name : string;
  opq_name_span : Span.span;
  opq_methods : extern_decl list;
  opq_span : Span.span;
}

type ext_lib_body = {
  elib_langs : lang_path list;
  elib_structs : foreign_struct list;
  elib_types : opaque_type list;
  elib_externs : extern_decl list;
}

(* Declared test blocks (see ast.ml for the shape commentary). *)
type value_head = { vh_segs : string list; vh_span : Span.span }

type test_value =
  | TvStr of string * Span.span
  | TvInt of int * Span.span
  | TvFloat of float * Span.span
  | TvBool of bool * Span.span
  | TvCtor of test_ctor
  | TvList of test_value list * Span.span
  | TvMap of ((string * Span.span) * test_value) list * Span.span
  | TvRef of { base : string; path : string list; ref_span : Span.span }
  | TvError of Span.span

and test_ctor = {
  tc_head : value_head;
  tc_fields : (string * Span.span * test_value) list;
  tc_span : Span.span;
}

type test_pattern =
  | TpCtor of test_pattern_ctor
  | TpLit of test_value
  | TpOk of Span.span
  | TpList of test_pattern list * Span.span
  | TpMap of {
      entries : ((string * Span.span) * test_pattern_field) list;
      map_open : bool;
      map_span : Span.span;
    }
  | TpError of Span.span

and test_pattern_ctor = {
  tp_head : value_head;
  tp_fields : (string * Span.span * test_pattern_field) list;
  tp_open : bool;
  tp_span : Span.span;
}

and test_pattern_field =
  | TpfPat of test_pattern
  | TpfAny of Span.span
  | TpfAbsent of Span.span

type stub_target = {
  st_binding : string;
  st_op : string;
  st_dep : string;
  st_span : Span.span;
}

type test_item =
  | TiConstruct of {
      bind : string;
      bind_span : Span.span;
      entry : string;
      entry_span : Span.span;
      fields : (string * Span.span * test_value) list;
      item_span : Span.span;
    }
  | TiStub of {
      bind : (string * Span.span) option;
      target : stub_target;
      value : test_value;
      item_span : Span.span;
    }
  | TiCall of {
      bind : string;
      bind_span : Span.span;
      recv : string;
      recv_span : Span.span;
      op : string;
      op_span : Span.span;
      input : test_value option;
      item_span : Span.span;
    }
  | TiExpect of {
      subject : string;
      subject_span : Span.span;
      requests : bool;
      pattern : test_pattern;
      item_span : Span.span;
    }

type decl_kind =
  | DStruct of { params : string list; members : member list; ops : decl list }
  | DEnum of { cases : enum_case list }
  | DUnion of { params : string list; variants : union_variant list }
  | DOp of { pname : string option; input : ty option; output : ty option }
  | DExt of {
      ekind : ext_kind;
      ekind_span : Span.span;
      esig : ext_sig option;
      eraw : Span.span option;
      ebindings : ext_binding list;
      econformance : string option;
    }
  | DExtLib of { body : ext_lib_body; span : Span.span }
  | DTest of { titems : test_item list }

and decl = {
  dname : string;
  dname_span : Span.span;
  pub : bool;
  dtraits : trait list;
  dkind : decl_kind;
}

type import = {
  imported_path : string list;
  alias : string option;
  ispan : Span.span;
}

type file = { imports : import list; decls : decl list }

val ty_span : ty -> Span.span
