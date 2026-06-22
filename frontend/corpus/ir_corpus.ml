(* Canonical IR documents used as the shared round-trip fixture corpus. The
   same models are emitted to JSON (the checked-in golden files) and decoded by
   the backend mirror, so this is the single arbiter both languages agree on. *)

open Tono_frontend

let member ?(required = true) ?default ?(constraints = []) ?(traits = []) name
    target : Ir.member =
  { name; target; required; default; constraints; traits }

let trait id value : Ir.trait = { trait_id = id; value }
let prim p = Ir.Prim p
let ref_ id args = Ir.Ref (id, args)
let string_t = prim Ir.String
let uuid_t = prim Ir.Uuid
let u32 = prim (Ir.int_prim ~bits:32 ~signed:false)
let u64 = prim (Ir.int_prim ~bits:64 ~signed:false)

(* Example: a native generic [Page[T]] plus an operation that returns
   [Page[Charge]] without any synthesized wrapper shape. *)
let list_charges : Ir.model =
  let page : Ir.shape =
    {
      id = "core#Page";
      kind =
        Ir.Structure
          {
            params = [ "T" ];
            members =
              [ member "items" (Ir.List (Ir.Param "T")); member "total" u32 ];
          };
      traits = [];
    }
  in
  let charge : Ir.shape =
    {
      id = "payments#Charge";
      kind =
        Ir.Structure
          {
            params = [];
            members =
              [
                member "id" uuid_t;
                member "amount" u64 ~constraints:[ Ir.range ~min:0. () ];
                member "currency" string_t
                  ~constraints:[ Ir.length ~min:3 ~max:3 () ];
              ];
          };
      traits = [];
    }
  in
  let list_charges_op : Ir.shape =
    {
      id = "payments#ListCharges";
      kind =
        Ir.Operation
          {
            input = None;
            output = Some (ref_ "core#Page" [ ref_ "payments#Charge" [] ]);
            errors = [];
          };
      traits = [];
    }
  in
  {
    tono_ir_version = Ir_json.current_ir_version;
    modules =
      [
        {
          mod_name = "payments";
          shapes = [ page; charge ];
          operations = [ list_charges_op ];
        };
      ];
  }

(* Example: nullability is two-state. [note] and [metadata] are nullable (T?);
   the others are present (T). *)
let nullable_charge : Ir.model =
  let charge : Ir.shape =
    {
      id = "payments#Charge";
      kind =
        Ir.Structure
          {
            params = [];
            members =
              [
                member "id" uuid_t;
                member "amount" u64;
                member "note" string_t ~required:false;
                member "metadata" (Ir.Map (string_t, string_t)) ~required:false;
              ];
          };
      traits = [];
    }
  in
  {
    tono_ir_version = Ir_json.current_ir_version;
    modules =
      [ { mod_name = "payments"; shapes = [ charge ]; operations = [] } ];
  }

(* Example: an enum (every enum is open) and a union whose variant wire tags are
   member names, overridable by a wire trait. *)
let open_enum_union : Ir.model =
  let status : Ir.shape =
    {
      id = "payments#Status";
      kind =
        Ir.Enum
          {
            backing = `String;
            values = [ ("active", None); ("closed", None); ("refunded", None) ];
          };
      traits = [];
    }
  in
  let source : Ir.shape =
    {
      id = "payments#Source";
      kind =
        Ir.union ~params:[]
          ~members:
            [
              member "card" (ref_ "payments#Card" []);
              member "bank" (ref_ "payments#Bank" [])
                ~traits:[ trait "core#wire" (`String "bank_account") ];
            ]
          ();
      traits = [];
    }
  in
  {
    tono_ir_version = Ir_json.current_ir_version;
    modules =
      [
        { mod_name = "payments"; shapes = [ status; source ]; operations = [] };
      ];
  }

(* Example: exercises every primitive, both container types, all core
   constraints, a default value, an int-backed enum, and arbitrary trait JSON
   (including a big integer that must stay exact). *)
let primitives : Ir.model =
  let i (bits, signed) = prim (Ir.int_prim ~bits ~signed) in
  let kitchen_sink : Ir.shape =
    {
      id = "lab#KitchenSink";
      kind =
        Ir.Structure
          {
            params = [];
            members =
              [
                member "b" (prim Ir.Bool);
                member "s" string_t;
                member "raw" (prim Ir.Bytes);
                member "i8" (i (8, true));
                member "i16" (i (16, true));
                member "i32" (i (32, true));
                member "i64" (i (64, true));
                member "u8" (i (8, false));
                member "u16" (i (16, false));
                member "u32" (i (32, false));
                member "u64" (i (64, false));
                member "f" (prim Ir.Float) ~constraints:[ Ir.multiple_of 0.5 ];
                member "ts" (prim Ir.Timestamp);
                member "day" (prim Ir.Date);
                member "dur" (prim Ir.Duration);
                member "tags" (Ir.List string_t)
                  ~constraints:[ Ir.length ~max:10 () ];
                member "attrs" (Ir.Map (string_t, string_t));
                member "score" (prim Ir.Float)
                  ~constraints:
                    [
                      Ir.range ~min:0. ~max:1. ~excl_min:false ~excl_max:true ();
                    ];
                member "name" string_t ~constraints:[ Ir.pattern "^[a-z]+$" ];
                member "kind" u32 ~default:(`Int 1);
                (* Small-magnitude float in a default position: the two emitters
                   format it differently as text, so this exercises the
                   data-level (not byte-level) cross-language comparison. *)
                member "rate" (prim Ir.Float) ~default:(`Float 1e-05);
                (* A present-but-null default is distinct from an absent one and
                   must survive the round-trip on both sides. *)
                member "hint" string_t ~required:false ~default:`Null;
              ];
          };
      traits =
        [
          trait "core#doc" (`String "Everything, once.");
          trait "lab#meta"
            (`Assoc
               [
                 (* Larger than i64 but within u64: exercises large-integer
                    fidelity that both sides must preserve without coercion. *)
                 ("count", `Intlit "12345678901234567890");
                 ("flags", `List [ `Bool true; `Null ]);
                 (* Float in an arbitrary trait-value position, emitter-sensitive
                    text, must agree as data across both sides. *)
                 ("ratio", `Float 1e-06);
               ]);
        ];
    }
  in
  let priority : Ir.shape =
    {
      id = "lab#Priority";
      kind =
        Ir.Enum
          { backing = `Int; values = [ ("low", Some 0); ("high", Some 10) ] };
      traits = [];
    }
  in
  {
    tono_ir_version = Ir_json.current_ir_version;
    modules =
      [
        {
          mod_name = "lab";
          shapes = [ kitchen_sink; priority ];
          operations = [];
        };
      ];
  }

(* Example: a service and an operation that exercise every operation field
   (input, output, and a non-empty errors list), so the cross-language contract
   pins the service kind and a fully-populated operation, not only the empty one. *)
let service_api : Ir.model =
  let request : Ir.shape =
    {
      id = "payments#ListChargesRequest";
      kind =
        Ir.Structure
          { params = []; members = [ member "limit" u32 ~required:false ] };
      traits = [];
    }
  in
  let not_found : Ir.shape =
    {
      id = "payments#NotFound";
      kind =
        Ir.Structure { params = []; members = [ member "message" string_t ] };
      traits = [];
    }
  in
  let list_charges_op : Ir.shape =
    {
      id = "payments#ListCharges";
      kind =
        Ir.Operation
          {
            input = Some (ref_ "payments#ListChargesRequest" []);
            output = Some (ref_ "core#Page" [ ref_ "payments#Charge" [] ]);
            errors = [ ref_ "payments#NotFound" [] ];
          };
      traits = [];
    }
  in
  let service : Ir.shape =
    {
      id = "payments#Payments";
      kind = Ir.Service { operations = [ "payments#ListCharges" ] };
      traits = [ trait "core#doc" (`String "Payments API.") ];
    }
  in
  {
    tono_ir_version = Ir_json.current_ir_version;
    modules =
      [
        {
          mod_name = "payments";
          shapes = [ request; not_found; service ];
          operations = [ list_charges_op ];
        };
      ];
  }

(* The full corpus, keyed by fixture file name. *)
let examples : (string * Ir.model) list =
  [
    ("list_charges", list_charges);
    ("nullable_charge", nullable_charge);
    ("open_enum_union", open_enum_union);
    ("primitives", primitives);
    ("service_api", service_api);
  ]
