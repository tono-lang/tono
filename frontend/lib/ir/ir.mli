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
      output : tref option;
      errors : tref list; (* tref so an operation can apply a generic directly *)
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

(* A parsed template: literal runs, entry-field placeholders ({.x}), and
   operation-input member placeholders ({id}, protocol trait positions only). *)
and template_part =
  | Tpl_lit of string
  | Tpl_field of string list
  | Tpl_input of string

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

(* One field of an entry or config; presence is governed by the sources. *)
and entry_field = {
  ef_name : string;
  ef_target : tref;
  ef_sources : source list;
  ef_format : template_part list option;
  ef_transforms : string list;
  ef_select : select option;
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
