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
  | Enum of {
      backing : [ `String | `Int ];
      values : (string * int option) list;
      open_ : bool;
    }
    (* open enums record the flag only; the Unknown
                                 variant is a backend decode-time concern *)
  | Service of { operations : shape_id list }
  | Operation of {
      input : tref option;
      output : tref option;
      errors : tref list;
    }
(* tref, so an operation can reference
                                            an applied generic directly *)

and shape = {
  id : shape_id;
  kind : shape_kind;
  traits : trait list; (* shape-level traits *)
}

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
