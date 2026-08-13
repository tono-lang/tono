(* Canonical intermediate representation shared by the frontend and the backend.
   These OCaml types are the single source of truth: the wire JSON encoding and
   the backend mirror are both derived from them and kept honest by round-trip
   tests. The model is always nominal -- every shape is named, every piece of
   metadata is a uniform trait. *)

(* Namespaced shape identity, e.g. "payments#Charge". Round-trips verbatim. *)
type shape_id = string

(* Closed primitive set. Sized integers use an explicit bit width and sign;
   there is deliberately no decimal (money is modeled as an integer of minor
   units). [Timestamp] and [Date] are distinct primitives. *)
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

(* Recursive type-application algebra. Generics are data, not names:
   [Page[Charge]] is [Ref ("...#Page", [Ref ("...#Charge", [])])] with no
   synthesized [PageOfCharge] shape. [args] is [] for a non-generic application. *)
type tref =
  | Prim of prim
  | Ref of shape_id * tref list
  | Param of string
  | List of tref
  | Map of tref * tref

(* Language -> "file#symbol" pointers for a custom constraint implementation. *)
type binding = (string * string) list

(* Core constraint vocabulary. [Custom] is carried here only as an in-memory
   convenience; on the wire and in a member it lives in the trait bag, never in
   the structured [constraints] field. *)
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

(* Arbitrary JSON, used for defaults and trait arguments. [Safe] keeps large
   integers exact via [`Intlit] instead of coercing them to floats. *)
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
      discriminator : string;
    }
    (* wire field name, default "type" *)
  | Enum of { backing : [ `String | `Int ]; values : enum_value list }
    (* every enum is open; the Unknown(raw) variant is a backend decode-time
       concern and is not materialized here *)
  | Service of { operations : shape_id list }
  | Operation of {
      input : tref option;
      input_name : string option;
          (* the declared parameter name; [None] for the unnamed legacy form,
             where [.input] has no user-given name to resolve *)
      output : tref option;
      errors : tref list;
      wire : wire_binding option;
    }
    (* tref, so an operation can reference an applied generic directly *)
  | Entry of { fields : entry_field list; operations : shape list }
    (* a struct with ops in its body: the construction surface of the generated
       SDK plus its methods. Never a wire type. *)
  | Config of { fields : entry_field list }
(* a struct that only participates in construction (its fields carry sources,
   or an entry composes it via @bind). Never a wire type. *)

and shape = {
  id : shape_id;
  kind : shape_kind;
  traits : trait list; (* shape-level traits *)
}

(* Where an entry/config field's value can come from, in declared order (the
   order is the fallback chain). *)
and source = Arg | With | Env of env_name | Default of json

(* The @env argument: a literal variable name, or a sibling-field reference
   whose resolved value names the variable. *)
and env_name = Env_name of string | Env_field of string list

(* One piece of a parsed template: a literal run, an entry-field placeholder
   ({.x} or {.x.y}), an operation-parameter placeholder ({.x} whose head
   resolves to the op's declared parameter name instead of an entry field;
   the segments after the head, [] for the whole parameter), or the legacy
   operation-input member placeholder ({id}, valid only in protocol trait
   positions such as @http path). *)
and template_part =
  | Tpl_lit of string
  | Tpl_field of string list
  | Tpl_param of string list
  | Tpl_input of string

(* The resolved HTTP binding a Protocol pass computes once and a Target reads
   directly. Constructors are [Wire_]-prefixed (and record fields
   [wb_]-prefixed) to stay unambiguous next to Protocol_http's own local
   response_part/value_expr types, the intermediate resolution this is built
   from, and because [method] is an OCaml keyword. *)
and wire_response_part =
  | Wire_response_header of string
  | Wire_response_status_code

and wire_value =
  | Wire_lit of json
  | Wire_field of string list
  | Wire_param of string list
    (* segments into the op's declared parameter; [] is the whole value *)
  | Wire_template of template_part list
  | Wire_object of (string * wire_value) list
(* a @body ctor mapper: struct-literal field name -> value *)

and wire_binding = {
  wb_method : string;
  wb_uri : wire_value;
      (* literal, entry/param reference, or template; {name}/{.field}
         placeholders parsed inside a template *)
  wb_body : wire_value option;
      (* @body(...): absent means the operation sends no body *)
  wb_response_bindings : (string * wire_response_part) list;
  wb_success : int list;
      (* status codes; the output type is always the op's own output *)
  wb_endpoint : wire_value option;
      (* @http endpoint: literal, ref, or template *)
  wb_request_headers : (template_part list * wire_value) list;
      (* @header(key, value): key template -> value *)
  wb_query : (template_part list * wire_value) list;
      (* @query(key, value): key template -> value *)
  wb_timeout : string list option;
      (* @timeout(.field): the entry-field path, unresolved *)
  wb_retry : string list option;
      (* @retry(.field): the entry-field path, unresolved *)
}

(* The selection table of [field: T = match .subject { ... }]. A pattern is a
   scalar JSON literal; [None] is the wildcard arm. *)
and select = { subject : string list; arms : select_arm list }
and select_arm = { arm_pattern : json option; arm_value : arm_value }

and arm_value =
  | Arm_field of string list
  | Arm_lit of json
  | Arm_sources of source list

(* One @bind(target, .source) at a composition point: the composed config's
   field being bound, and the entry field path feeding it. *)
and bind = { bind_field : string; bind_source : string list }

(* One argument to an extern call (see [entry_call]/[extern_lang] below): the
   caller's own parameter by name, a field-reference path, a struct-literal
   mapper, a scalar literal, a list, or a nested call -- the last three only
   arise inside a ctor field's value (e.g. [opts { retries: 3 }]), never as a
   bare top-level call argument. Shared by a field's own call source, a
   language block's [call:] line, and a trait-argument call. *)
and call_arg =
  | Ca_param of string
  | Ca_ref of string list
  | Ca_ctor of call_ctor
  | Ca_lit of json
  | Ca_list of call_arg list
  | Ca_call of entry_call

and call_ctor = { cc_name : string; cc_fields : (string * call_arg) list }

(* A field's [= ns.fn(args)] value: a call into an [extern] declared in the
   [ext] block named [ec_ns]. Resolving [ec_ns]/[ec_fn] against a declared
   extern is deferred (out of scope; see [Ir.ext_lib]). *)
and entry_call = { ec_ns : string; ec_fn : string; ec_args : call_arg list }

(* One field of an entry or config. Presence is governed by the sources (there
   is no required/default pair: @default is a source, optionality is @with). *)
and entry_field = {
  ef_name : string;
  ef_target : tref;
  ef_sources : source list;
  ef_format : template_part list option; (* @format derivation *)
  ef_transforms : string list; (* @str::* pipeline, in declared order *)
  ef_select : select option; (* [= match] selection *)
  ef_call : entry_call option; (* [= ns.fn(args)] extern-call selection *)
  ef_binds : bind list; (* composition bindings; config-typed fields only *)
  ef_constraints : constraint_ list;
  ef_traits : trait list;
}

(* One member of an enum: its wire name, an optional explicit integer (present
   only on int-backed enums), and its trait bag. Documentation (@doc) rides the
   bag exactly like it does on shapes and struct members, so the codegen reads
   it through the same path everywhere. *)
and enum_value = {
  ev_name : string;
  ev_int : int option;
  ev_traits : trait list;
}

(* A bespoke extension: logic that does not fit the pure calculus, bound to a
   per-language source file. [Hook] fills a fixed lifecycle slot (its name is the
   slot); [Contract] and [Constraint] are named with a typed signature; [Impl]
   implements the operation its name points at, taking that operation's signature.
   The binding is the escape hatch; conformance gates a contract at emit time. *)
type ext_kind = Hook | Contract | Constraint | Impl

(* The typed boundary of a contract/constraint. Hooks and impls omit it: a hook's
   signature is fixed by its slot, an impl's by the operation it names. Signature
   refs are stored verbatim, not resolved against user shapes
   (binding-vs-signature validation is deferred). *)
type ext_sig = { input : tref; output : tref }

type extension = {
  ext_name : string;
      (* slot name for a hook, the operation name (bare or "entry.op") for an
         impl, otherwise the contract name *)
  ext_kind : ext_kind;
  ext_sig : ext_sig option;
  ext_raw : bool;
      (* impl only: the bound symbol returns a raw outcome the generated glue
         decodes, instead of the operation's declared output *)
  ext_bindings : binding; (* lang -> "ext/{lang}/...#symbol" *)
  ext_conformance : string option; (* mandatory for a contract at emit time *)
}

(* ── FFI library declarations: ext <name> { ... } ──────────────────────────
   The new [ext] form, distinct from [extension] above (hook/contract/
   constraint/impl). Surface-and-IR only for now: no typecheck resolves an
   extern's arity/types, an [errors:] sentinel against a declared error shape,
   or a [returns:] field ref against its [yields:] name. That resolution, and
   codegen of the call itself, are later passes. *)

(* One [yields:] position; [yp_type] is [None] only for the reserved [error]
   sentinel ([yp_is_error]), never for an ordinary omitted type. *)
type yields_pos = { yp_name : string; yp_type : tref option; yp_is_error : bool }

(* A [returns:] field value: a bare ref into a yields-bound name, or a match
   over one -- the same shape a member's own [= match] selection uses. *)
type returns_value = Rv_ref of string list | Rv_select of select
type returns_field = { rvf_name : string; rvf_value : returns_value }
type returns_lit = { rvl_type : tref; rvl_fields : returns_field list }

type error_binding = { erb_sentinel : string; erb_type : string }

(* One per-language block inside an [extern]'s body. *)
type extern_lang = {
  el_lang : string;
  el_symbol : string;
  el_call_args : call_arg list;
  el_yields : yields_pos list; (* [] when no yields: line *)
  el_returns : returns_lit option;
  el_errors : error_binding list;
}

type extern_param = { xp_name : string; xp_type : tref }

(* A free function inside an [ext] block, or a method inside a [type] opaque
   handle. *)
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
  xl_externs : extern_decl list; (* free (non-method) externs *)
}

(* ── Declared tests ────────────────────────────────────────────────────── *)

(* The declared dependency a stub replaces: the protocol seam of an @http
   operation, or the bound symbol of an "ext impl" operation. *)
type test_dep = Dep_http | Dep_impl

(* One construction binding: [c: client { field: value }]. Values are in wire
   form (i64/u64 as strings, wire keys); an omitted field runs its declared
   chain. Entry and operation names are bare (module-local), like the surface. *)
type test_construction = {
  tc_binding : string;
  tc_entry : string;
  tc_values : (string * json) list;
}

(* One canned answer of a stub. The http form carries the response triple; the
   impl forms carry the operation's own language: a typed output value, a
   declared error by shape name, or the undeclared-failure contract wrap. *)
type stub_answer =
  | Answer_http of {
      ans_status : int;
      ans_headers : (string * string) list;
      ans_body : string;
    }
  | Answer_value of json
  | Answer_error of { ans_shape : string; ans_data : json }
  | Answer_contract

(* One stub binding. [ts_binding] is [None] for the anonymous form (which no
   expect can reference). A multi-answer list is a sequence: each call consumes
   the next answer and the last repeats. *)
type test_stub = {
  ts_binding : string option;
  ts_client : string;
  ts_op : string;
  ts_dep : test_dep;
  ts_answers : stub_answer list;
}

(* One call binding: [got: c.get_user(input)]. The input is wire-form JSON in
   which a dataflow leaf is spelled {"$ref":{"binding":...,"path":[...]}}. *)
type test_call = {
  call_binding : string;
  call_client : string;
  call_op : string;
  call_input : json option;
}

(* A field position inside a pattern: a nested pattern, asserted absence
   ([None]), or asserted presence with a free value ([any]). *)
type field_pattern = Fp_pat of test_pattern | Fp_absent | Fp_present

(* An expect pattern. [P_eq] compares wire JSON; the structural forms carry the
   shape (or taxonomy category), whether uncited fields are free ([..]), and
   the cited fields. *)
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
          (* api|validation|decode|contract|config|transport *)
      pt_open : bool;
      pt_fields : (string * field_pattern) list;
    }
  | P_ok

(* A pattern over one recorded request of an http stub. Headers match as a
   subset by name (case handling is the backend's concern). *)
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

(* One declared test, grouped by construct kind; each group keeps declaration
   order, and every reference points backwards. *)
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
   width outside the closed set, or a non-finite float that has no JSON form).
   The smart constructors below prevent constructing such values in the first
   place. *)
exception Invalid_ir of string

let valid_int_bits = [ 8; 16; 32; 64 ]

let int_prim ~bits ~signed =
  if not (List.mem bits valid_int_bits) then
    raise
      (Invalid_ir
         (Printf.sprintf "integer bit width %d is not one of 8, 16, 32, 64" bits));
  Int { bits; signed }

(* JSON numbers cannot encode NaN or infinities, so they are rejected where a
   float reaches the wire. *)
let finite what f =
  if not (Float.is_finite f) then
    raise (Invalid_ir (Printf.sprintf "%s must be a finite number" what));
  f

let range ?min ?max ?(excl_min = false) ?(excl_max = false) () =
  Option.iter (fun f -> ignore (finite "Range bound" f)) min;
  Option.iter (fun f -> ignore (finite "Range bound" f)) max;
  Range { min; max; excl_min; excl_max }

let length ?min ?max () = Length { min; max }
let pattern s = Pattern s

let multiple_of f =
  ignore (finite "MultipleOf" f);
  MultipleOf f

(* A union always carries an explicit discriminator so the field is present in
   the IR even when the surface syntax omitted it. *)
let union ?(discriminator = "type") ~params ~members () =
  Union { params; members; discriminator }

let enum_value ?int ?(traits = []) name =
  { ev_name = name; ev_int = int; ev_traits = traits }

module Shape_map = Map.Make (String)

(* In-memory model plus an index from namespaced id to shape, used by the
   typechecker and later passes. *)
type indexed_model = { meta : model; by_id : shape Shape_map.t }

let index_model (m : model) : indexed_model =
  let add acc (s : shape) = Shape_map.add s.id s acc in
  let by_id =
    List.fold_left
      (fun acc modl ->
        let acc = List.fold_left add acc modl.shapes in
        List.fold_left add acc modl.operations)
      Shape_map.empty m.modules
  in
  { meta = m; by_id }
