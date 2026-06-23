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
      values : (string * int option) list;
          (* every enum is open; Unknown(raw) is a backend decode-time concern *)
    }
  | Service of { operations : shape_id list }
  | Operation of {
      input : tref option;
      output : tref option;
      errors : tref list; (* tref so an operation can apply a generic directly *)
    }

and shape = { id : shape_id; kind : shape_kind; traits : trait list }

type module_ = {
  mod_name : string;
  shapes : shape list;
  operations : shape list;
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

module Shape_map : Map.S with type key = shape_id

(* In-memory model plus an index from namespaced id to shape. *)
type indexed_model = { meta : model; by_id : shape Shape_map.t }

val index_model : model -> indexed_model
