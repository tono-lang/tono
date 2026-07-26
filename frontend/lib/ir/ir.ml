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
      output : tref option;
      errors : tref list;
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
   ({.x} or {.x.y}), or an operation-input member placeholder ({id}, valid only
   in protocol trait positions such as @http path). *)
and template_part =
  | Tpl_lit of string
  | Tpl_field of string list
  | Tpl_input of string

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

(* One field of an entry or config. Presence is governed by the sources (there
   is no required/default pair: @default is a source, optionality is @with). *)
and entry_field = {
  ef_name : string;
  ef_target : tref;
  ef_sources : source list;
  ef_format : template_part list option; (* @format derivation *)
  ef_transforms : string list; (* @str::* pipeline, in declared order *)
  ef_select : select option; (* [= match] selection *)
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
   slot); [Contract] and [Constraint] are named with a typed signature. The
   binding is the escape hatch; conformance gates a contract at emit time. *)
type ext_kind = Hook | Contract | Constraint

(* The typed boundary of a contract/constraint. Hooks omit it: their signature is
   fixed by the slot. Signature refs are stored verbatim, not resolved against
   user shapes (binding-vs-signature validation is deferred). *)
type ext_sig = { input : tref; output : tref }

type extension = {
  ext_name : string; (* slot name for a hook, otherwise the contract name *)
  ext_kind : ext_kind;
  ext_sig : ext_sig option;
  ext_bindings : binding; (* lang -> "ext/{lang}/...#symbol" *)
  ext_conformance : string option; (* mandatory for a contract at emit time *)
}

type module_ = {
  mod_name : string;
  shapes : shape list;
  operations : shape list;
  extensions : extension list;
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
