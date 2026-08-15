(* Canonical intermediate representation shared by the frontend and the backend.
   These OCaml types are the single source of truth; the wire JSON encoding and
   the backend mirror are derived from them. Every shape is named and every piece
   of metadata is a uniform trait. *)

(* Namespaced shape identity, e.g. "payments#Charge". *)
type shape_id = string

(* Closed primitive set; sized integers carry a bit width and a sign. There is
   deliberately no decimal. [Timestamp] and [Date] are distinct. *)
type prim =
  | Bool
  | String
  | Bytes
  | Int of { bits : int; signed : bool } (* bits is one of 8, 16, 32, 64 *)
  | Float
  | Timestamp
  | Date
  | Duration
  | Uuid

(* Recursive type-application algebra. Generics are data, not names; [args] is []
   for a non-generic application. *)
type tref =
  | Prim of prim
  | Ref of shape_id * tref list
  | Param of string
  | List of tref
  | Map of tref * tref

(* Language -> "file#symbol" pointers for a custom constraint implementation. *)
type binding = (string * string) list

(* Core constraint vocabulary. [Custom] is an in-memory convenience only; it
   belongs in the trait bag on the wire, never in the structured field. *)
type constraint_ =
  | Range of {
      min : float option;
      max : float option;
      excl_min : bool;
      excl_max : bool;
    }
  | Length of { min : int option; max : int option }
  | Pattern of string
  | MultipleOf of float
  | Custom of { name : string; binding : binding }

(* Arbitrary JSON for defaults and trait arguments. [Safe] keeps large integers
   exact via [`Intlit] instead of coercing them to floats. *)
type json = Yojson.Safe.t

type member = {
  name : string;
  target : tref;
  required : bool; (* false denotes a nullable T?; null <> absent *)
  default : json option; (* present => optional in the API, always sent *)
  constraints : constraint_ list; (* core vocabulary only *)
  traits : trait list; (* non-core extensions and custom constraints *)
}

and trait = { trait_id : shape_id; value : json }

and shape_kind =
  | Structure of { params : string list; members : member list }
  | Union of {
      params : string list;
      members : member list;
      discriminator : string; (* wire field name, default "type" *)
    }
  | Enum of {
      backing : [ `String | `Int ];
      values : enum_value list;
          (* every enum is open; Unknown(raw) is a backend decode-time concern *)
    }
  | Service of { operations : shape_id list }
  | Operation of {
      input : tref option;
      input_name : string option;
      output : tref option;
      errors : tref list;
          (* tref so an operation can apply a generic directly *)
      wire : wire_binding option;
      impl_call : op_impl_call option;
          (* the op's own "impl .field.method(args)" body (RFC-0023): a third
             implementation source alongside [wire] and a legacy "ext impl"
             extension. Resolving the receiver against a declared opaque
             handle and the method against its declared "extern" is a
             typechecker concern; this only carries the call structured. No
             backend reads it yet. *)
    }
  | Entry of { fields : entry_field list; operations : shape list }
    (* a struct with ops in its body: the SDK construction surface plus its
       methods; never a wire type *)
  | Config of { fields : entry_field list }
(* a construction-only struct (fields carry sources, or an entry composes
       it via @bind); never a wire type *)

and shape = { id : shape_id; kind : shape_kind; traits : trait list }

(* Where an entry/config field's value can come from; the declared order is the
   fallback chain. *)
and source = Arg | With | Env of env_name | Default of json
and env_name = Env_name of string | Env_field of string list

(* A parsed template: literal runs, entry-field placeholders ({.x}),
   operation-parameter placeholders ({.x} whose head resolves to the op's
   declared parameter name), and the legacy operation-input member
   placeholders ({id}, protocol trait positions only). *)
and template_part =
  | Tpl_lit of string
  | Tpl_field of string list
  | Tpl_param of string list
  | Tpl_input of string

(* The resolved HTTP binding a Protocol pass computes once and a Target reads
   directly. *)
and wire_response_part =
  | Wire_response_header of string
  | Wire_response_status_code

and wire_value =
  | Wire_lit of json
  | Wire_field of string list
  | Wire_param of string list
  | Wire_template of template_part list
  | Wire_object of (string * wire_value) list
  | Wire_call of wire_call

(* An extern call read as a @header/@body value: [wcl_args] mirror an
   ordinary extern call's arguments, plus the reserved [Wca_request] marker
   for ".request", the canonical, already-assembled request -- legal only
   here, never during entry construction (see [Ir.entry_call] and
   [Wca_request]'s own comment). *)
and wire_call = { wcl_ns : string; wcl_fn : string; wcl_args : wire_call_arg list }

and wire_call_arg =
  | Wca_field of string list
  | Wca_param of string list
  | Wca_lit of json
  | Wca_ctor of (string * wire_call_arg) list
  | Wca_request

and wire_binding = {
  wb_method : string;
  wb_uri : wire_value;
  wb_body : wire_value option;
  wb_response_bindings : (string * wire_response_part) list;
  wb_success : int list;
  wb_endpoint : wire_value option;
  wb_request_headers : (template_part list * wire_value) list;
  wb_query : (template_part list * wire_value) list;
  wb_timeout : string list option;
  wb_retry : string list option;
}

(* The selection table of [field: T = match .subject { ... }]; a pattern is a
   scalar JSON literal, [None] the wildcard arm. *)
and select = { subject : string list; arms : select_arm list }
and select_arm = { arm_pattern : json option; arm_value : arm_value }

and arm_value =
  | Arm_field of string list
  | Arm_lit of json
  | Arm_sources of source list

(* One @bind(target, .source) at a composition point. *)
and bind = { bind_field : string; bind_source : string list }

(* One argument to an extern call: the caller's own parameter by name, a
   field-reference path, a struct-literal mapper, a scalar literal, a list, or
   a nested call (the last three only arise inside a ctor field's value). *)
and call_arg =
  | Ca_param of string
  | Ca_ref of string list
  | Ca_ctor of call_ctor
  | Ca_lit of json
  | Ca_list of call_arg list
  | Ca_call of entry_call

and call_ctor = { cc_name : string; cc_fields : (string * call_arg) list }

(* A field's [= ns.fn(args)] value: a call into an extern declared in the ext
   block named [ec_ns]. Resolving it against a declared extern is deferred. *)
and entry_call = { ec_ns : string; ec_fn : string; ec_args : call_arg list }

(* An op's own [impl .field.method(args)] body: the receiver is a field path
   (an entry field, an opaque handle), not a bare "ext" namespace, so this
   mirrors [entry_call] with [oic_recv : string list] in place of [ec_ns].
   Resolving the receiver/method against a declared handle is deferred to
   the typechecker. *)
and op_impl_call = {
  oic_recv : string list;
  oic_method : string;
  oic_args : call_arg list;
}

(* One field of an entry or config; presence is governed by the sources. *)
and entry_field = {
  ef_name : string;
  ef_target : tref;
  ef_sources : source list;
  ef_format : template_part list option;
  ef_transforms : string list;
  ef_select : select option;
  ef_call : entry_call option;
  ef_binds : bind list;
  ef_constraints : constraint_ list;
  ef_traits : trait list;
}

(* One enum member: wire name, optional explicit integer (int-backed enums only),
   and its trait bag. Documentation (@doc) rides the bag, as on shapes and members. *)
and enum_value = {
  ev_name : string;
  ev_int : int option;
  ev_traits : trait list;
}

(* A bespoke extension bound to a per-language source file. [Hook] fills a fixed
   lifecycle slot (its name is the slot); [Contract] and [Constraint] are named
   with a typed signature; [Impl] implements the operation its name points at.
   Conformance gates a contract at emit time. *)
type ext_kind = Hook | Contract | Constraint | Impl

(* The typed boundary of a contract/constraint. Hooks and impls omit it: their
   signature is fixed by the slot or by the named operation. Signature refs are
   stored verbatim, not resolved. *)
type ext_sig = { input : tref; output : tref }

type extension = {
  ext_name : string;
  ext_kind : ext_kind;
  ext_sig : ext_sig option;
  ext_raw : bool;
  ext_bindings : binding;
  ext_conformance : string option;
}

(* FFI library declarations: ext <name> { ... } (see ir.ml for commentary). *)
type yields_pos = {
  yp_name : string;
  yp_type : tref option;
  yp_is_error : bool;
}

type returns_value = Rv_ref of string list | Rv_select of select
type returns_field = { rvf_name : string; rvf_value : returns_value }
type returns_lit = { rvl_type : tref; rvl_fields : returns_field list }
type error_binding = { erb_sentinel : string; erb_type : string }

type extern_lang = {
  el_lang : string;
  el_symbol : string;
  el_call_args : call_arg list;
  el_yields : yields_pos list;
  el_returns : returns_lit option;
  el_errors : error_binding list;
  el_sync : bool;
}

type extern_param = { xp_name : string; xp_type : tref }

type extern_decl = {
  x_name : string;
  x_params : extern_param list;
  x_return : tref;
  x_langs : extern_lang list;
}

type foreign_field = { fgf_name : string; fgf_type : tref }
type foreign_struct = { fgs_name : string; fgs_fields : foreign_field list }
type opaque_type = { opq_name : string; opq_methods : extern_decl list }
type lang_path = { lgp_lang : string; lgp_path : string }

type ext_lib = {
  xl_name : string;
  xl_langs : lang_path list;
  xl_structs : foreign_struct list;
  xl_types : opaque_type list;
  xl_externs : extern_decl list;
}

(* Declared tests (see ir.ml for the field commentary). *)
type test_dep = Dep_http | Dep_impl

type test_construction = {
  tc_binding : string;
  tc_entry : string;
  tc_values : (string * json) list;
}

type stub_answer =
  | Answer_http of {
      ans_status : int;
      ans_headers : (string * string) list;
      ans_body : string;
    }
  | Answer_value of json
  | Answer_error of { ans_shape : string; ans_data : json }
  | Answer_contract

type test_stub = {
  ts_binding : string option;
  ts_client : string;
  ts_op : string;
  ts_dep : test_dep;
  ts_answers : stub_answer list;
}

type test_call = {
  call_binding : string;
  call_client : string;
  call_op : string;
  call_input : json option;
}

type field_pattern = Fp_pat of test_pattern | Fp_absent | Fp_present

and test_pattern =
  | P_eq of json
  | P_struct of {
      ps_shape : string;
      ps_open : bool;
      ps_fields : (string * field_pattern) list;
    }
  | P_error of {
      pe_shape : string;
      pe_open : bool;
      pe_fields : (string * field_pattern) list;
    }
  | P_taxonomy of {
      pt_category : string;
      pt_open : bool;
      pt_fields : (string * field_pattern) list;
    }
  | P_ok

type request_pattern = {
  rp_open : bool;
  rp_fields : (string * field_pattern) list;
  rp_headers : (string * field_pattern) list option;
}

type test_expect =
  | Expect_outcome of { ex_subject : string; ex_pattern : test_pattern }
  | Expect_requests of {
      ex_subject : string;
      ex_requests : request_pattern list;
    }

type test_decl = {
  t_name : string;
  t_constructions : test_construction list;
  t_stubs : test_stub list;
  t_calls : test_call list;
  t_expects : test_expect list;
}

type module_ = {
  mod_name : string;
  shapes : shape list;
  operations : shape list;
  extensions : extension list;
  ext_libs : ext_lib list;
  tests : test_decl list;
}

type model = {
  tono_ir_version : int; (* monotonic integer gate, not semver *)
  modules : module_ list;
}

(* Raised when an in-memory value cannot be represented on the wire (an integer
   width outside the closed set, or a non-finite float). The smart constructors
   prevent building such values. *)
exception Invalid_ir of string

(* The admissible integer bit widths: 8, 16, 32, 64. *)
val valid_int_bits : int list

(* [int_prim ~bits ~signed] raises [Invalid_ir] unless [bits] is in
   [valid_int_bits]. *)
val int_prim : bits:int -> signed:bool -> prim

(* [finite what f] returns [f], or raises [Invalid_ir] if [f] is NaN/infinite. *)
val finite : string -> float -> float

(* Smart constructors for the core constraints; the float-bearing ones reject
   non-finite bounds. *)
val range :
  ?min:float ->
  ?max:float ->
  ?excl_min:bool ->
  ?excl_max:bool ->
  unit ->
  constraint_

val length : ?min:int -> ?max:int -> unit -> constraint_
val pattern : string -> constraint_
val multiple_of : float -> constraint_

(* A union always carries an explicit discriminator (default "type"). *)
val union :
  ?discriminator:string ->
  params:string list ->
  members:member list ->
  unit ->
  shape_kind

(* [enum_value ?int ?traits name] builds an enum member; [int] is the explicit
   discriminant (int-backed enums), [traits] its bag (defaults empty). *)
val enum_value : ?int:int -> ?traits:trait list -> string -> enum_value

module Shape_map : Map.S with type key = shape_id

(* In-memory model plus an index from namespaced id to shape. *)
type indexed_model = { meta : model; by_id : shape Shape_map.t }

val index_model : model -> indexed_model
