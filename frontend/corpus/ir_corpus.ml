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
          extensions = [];
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
      [
        {
          mod_name = "payments";
          shapes = [ charge ];
          operations = [];
          extensions = [];
        };
      ];
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
            values =
              [
                Ir.enum_value "active";
                Ir.enum_value "closed";
                Ir.enum_value "refunded";
              ];
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
        {
          mod_name = "payments";
          shapes = [ status; source ];
          operations = [];
          extensions = [];
        };
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
          {
            backing = `Int;
            values =
              [ Ir.enum_value ~int:0 "low"; Ir.enum_value ~int:10 "high" ];
          };
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
          extensions = [];
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
          extensions = [];
        };
      ];
  }

(* Example: the bespoke extension table. A lifecycle hook (no signature), a
   signing contract (typed signature plus a mandatory conformance reference), and
   a custom constraint, each bound to per-language source files. *)
let extension_model : Ir.model =
  let before_request : Ir.extension =
    {
      ext_name = "before_request";
      ext_kind = Ir.Hook;
      ext_sig = None;
      ext_raw = false;
      ext_bindings =
        [
          ("ts", "ext/ts/auth.ts#addBearer");
          ("rust", "ext/rust/auth.rs#add_bearer");
        ];
      ext_conformance = None;
    }
  in
  let sign_request : Ir.extension =
    {
      ext_name = "sign_request";
      ext_kind = Ir.Contract;
      ext_sig = Some { input = ref_ "canonical_request" []; output = string_t };
      ext_raw = false;
      ext_bindings =
        [
          ("ts", "ext/ts/sign.ts#signRequest");
          ("go", "ext/go/sign.go#SignRequest");
        ];
      ext_conformance = Some "vectors/sign_request.json";
    }
  in
  let luhn : Ir.extension =
    {
      ext_name = "luhn";
      ext_kind = Ir.Constraint;
      ext_sig = Some { input = string_t; output = prim Ir.Bool };
      ext_raw = false;
      ext_bindings = [ ("ts", "ext/ts/luhn.ts#isLuhn") ];
      ext_conformance = None;
    }
  in
  {
    tono_ir_version = Ir_json.current_ir_version;
    modules =
      [
        {
          mod_name = "payments";
          shapes = [];
          operations = [];
          extensions = [ before_request; sign_request; luhn ];
        };
      ];
  }

(* Example: the entry model surface (v5). One entry with every source kind,
   a @format derivation, a transform pipeline, a match selection covering all
   arm forms, a @bind composition, and a nested op whose traits carry field
   references; one config; the wire structs the op touches. *)
let entries_client : Ir.model =
  let field ?(sources = []) ?format ?(transforms = []) ?select ?(binds = [])
      ?(constraints = []) ?(traits = []) name target : Ir.entry_field =
    {
      ef_name = name;
      ef_target = target;
      ef_sources = sources;
      ef_format = format;
      ef_transforms = transforms;
      ef_select = select;
      ef_binds = binds;
      ef_constraints = constraints;
      ef_traits = traits;
    }
  in
  let conf : Ir.shape =
    {
      id = "notes#conf";
      kind =
        Ir.Config
          {
            fields =
              [
                field "api_key" string_t
                  ~sources:[ Ir.Env (Ir.Env_name "API_KEY") ];
                field "region" string_t
                  ~sources:
                    [ Ir.Env (Ir.Env_name "REGION"); Ir.Default (`String "us") ];
              ];
          };
      traits = [];
    }
  in
  let save_note : Ir.shape =
    {
      id = "notes#client.save_note";
      kind =
        Ir.Operation
          {
            input = Some (ref_ "notes#note" []);
            output = Some (ref_ "notes#note" []);
            errors = [ ref_ "notes#overloaded" [] ];
          };
      traits =
        [
          trait "http"
            (`Assoc
               [
                 ("method", `String "POST");
                 ("path", `String "/notes/{id}");
                 ("endpoint", `Assoc [ ("field", `List [ `String "endpoint" ]) ]);
               ]);
          trait "header"
            (`List
               [
                 `String "X-Client-Name";
                 `Assoc [ ("field", `List [ `String "client_name" ]) ];
               ]);
          trait "timeout"
            (`List [ `Assoc [ ("field", `List [ `String "timeout" ]) ] ]);
          trait "retry"
            (`List [ `Assoc [ ("field", `List [ `String "max_retries" ]) ] ]);
        ];
    }
  in
  let client : Ir.shape =
    {
      id = "notes#client";
      kind =
        Ir.Entry
          {
            fields =
              [
                field "api_key" string_t ~sources:[ Ir.Arg ]
                  ~constraints:[ Ir.length ~min:1 () ];
                field "client_name" string_t
                  ~sources:[ Ir.With; Ir.Default (`String "demo") ]
                  ~traits:
                    [ trait "doc" (`List [ `String "Names the caller." ]) ];
                field "client_key" string_t
                  ~format:[ Ir.Tpl_field [ "client_name" ] ]
                  ~transforms:[ "trim"; "upper_snake" ];
                field "endpoint_env" string_t
                  ~format:
                    [
                      Ir.Tpl_lit "ENDPOINT_";
                      Ir.Tpl_field [ "client_key" ];
                      Ir.Tpl_lit "_V2";
                    ];
                field "endpoint_version" string_t
                  ~sources:
                    [
                      Ir.Env (Ir.Env_name "ENDPOINT_VERSION");
                      Ir.Default (`String "v2");
                    ];
                field "endpoint_v1" string_t
                  ~sources:[ Ir.Env (Ir.Env_name "ENDPOINT") ];
                field "endpoint_v2" string_t
                  ~sources:[ Ir.Env (Ir.Env_field [ "endpoint_env" ]) ];
                field "endpoint" string_t
                  ~select:
                    {
                      Ir.subject = [ "endpoint_version" ];
                      arms =
                        [
                          {
                            arm_pattern = Some (`String "v1");
                            arm_value = Ir.Arm_field [ "endpoint_v1" ];
                          };
                          {
                            arm_pattern = Some (`String "legacy");
                            arm_value =
                              Ir.Arm_lit (`String "https://old.example.com");
                          };
                          {
                            arm_pattern = None;
                            arm_value =
                              Ir.Arm_sources
                                [
                                  Ir.Env (Ir.Env_name "ENDPOINT_V2");
                                  Ir.Default (`String "https://example.com");
                                ];
                          };
                        ];
                    };
                field "timeout" (prim Ir.Duration)
                  ~sources:[ Ir.With; Ir.Default (`String "10s") ];
                field "max_retries"
                  (prim (Ir.int_prim ~bits:32 ~signed:true))
                  ~sources:[ Ir.With; Ir.Default (`Int 3) ];
                field "settings" (ref_ "notes#conf" [])
                  ~binds:
                    [
                      { Ir.bind_field = "api_key"; bind_source = [ "api_key" ] };
                    ];
              ];
            operations = [ save_note ];
          };
      traits = [ trait "pub" `Null ];
    }
  in
  let note : Ir.shape =
    {
      id = "notes#note";
      kind =
        Ir.Structure
          {
            params = [];
            members =
              [
                member "id" string_t ~traits:[ trait "httpLabel" `Null ];
                member "body" string_t;
              ];
          };
      traits = [];
    }
  in
  let overloaded : Ir.shape =
    {
      id = "notes#overloaded";
      kind =
        Ir.Structure { params = []; members = [ member "message" string_t ] };
      traits =
        [
          trait "status" (`List [ `Int 529 ]);
          trait "errorCode" (`List [ `String "overloaded" ]);
          trait "retryable" `Null;
        ];
    }
  in
  {
    tono_ir_version = Ir_json.current_ir_version;
    modules =
      [
        {
          mod_name = "notes";
          shapes = [ conf; client; note; overloaded ];
          operations = [];
          extensions = [];
        };
      ];
  }

(* Example: a hybrid entry (v6). One operation reaches a protocol through @http,
   one is implemented by bespoke sources in the typed form, and one in the raw
   form where the bound symbol returns an outcome the generated glue decodes.
   The impl extensions name their operations in the qualified "entry.op" form. *)
let bespoke_impl : Ir.model =
  let field ?(sources = []) name target : Ir.entry_field =
    {
      ef_name = name;
      ef_target = target;
      ef_sources = sources;
      ef_format = None;
      ef_transforms = [];
      ef_select = None;
      ef_binds = [];
      ef_constraints = [];
      ef_traits = [];
    }
  in
  let op id ~input ~output ~traits : Ir.shape =
    {
      id;
      kind =
        Ir.Operation
          {
            input = Some (ref_ input []);
            output = Some (ref_ output []);
            errors = [ ref_ "notes#overloaded" [] ];
          };
      traits;
    }
  in
  let fetch_note =
    op "notes#client.fetch_note" ~input:"notes#note_ref" ~output:"notes#note"
      ~traits:
        [
          trait "http"
            (`Assoc
               [
                 ("method", `String "GET");
                 ("path", `String "/notes/{id}");
                 ("endpoint", `Assoc [ ("field", `List [ `String "endpoint" ]) ]);
               ]);
        ]
  in
  let save_note =
    op "notes#client.save_note" ~input:"notes#note" ~output:"notes#note"
      ~traits:[]
  in
  let search_notes =
    op "notes#client.search_notes" ~input:"notes#note_ref" ~output:"notes#note"
      ~traits:[]
  in
  let client : Ir.shape =
    {
      id = "notes#client";
      kind =
        Ir.Entry
          {
            fields =
              [
                field "api_key" string_t ~sources:[ Ir.Arg ];
                field "endpoint" string_t
                  ~sources:
                    [
                      Ir.Env (Ir.Env_name "NOTES_ENDPOINT");
                      Ir.Default (`String "https://example.com");
                    ];
              ];
            operations = [ fetch_note; save_note; search_notes ];
          };
      traits = [ trait "pub" `Null ];
    }
  in
  let note : Ir.shape =
    {
      id = "notes#note";
      kind =
        Ir.Structure
          {
            params = [];
            members = [ member "id" string_t; member "body" string_t ];
          };
      traits = [];
    }
  in
  let note_ref : Ir.shape =
    {
      id = "notes#note_ref";
      kind =
        Ir.Structure
          {
            params = [];
            members =
              [ member "id" string_t ~traits:[ trait "httpLabel" `Null ] ];
          };
      traits = [];
    }
  in
  let overloaded : Ir.shape =
    {
      id = "notes#overloaded";
      kind =
        Ir.Structure { params = []; members = [ member "message" string_t ] };
      traits =
        [
          trait "status" (`List [ `Int 529 ]);
          trait "errorCode" (`List [ `String "overloaded" ]);
          trait "retryable" `Null;
        ];
    }
  in
  let impl name ~raw ~conformance : Ir.extension =
    {
      ext_name = name;
      ext_kind = Ir.Impl;
      ext_sig = None;
      ext_raw = raw;
      ext_bindings =
        [ ("go", "ext/go/notes.go#" ^ name); ("ts", "ext/ts/notes.ts#" ^ name) ];
      ext_conformance = conformance;
    }
  in
  {
    tono_ir_version = Ir_json.current_ir_version;
    modules =
      [
        {
          mod_name = "notes";
          shapes = [ client; note; note_ref; overloaded ];
          operations = [];
          extensions =
            [
              impl "client.save_note" ~raw:false
                ~conformance:(Some "vectors/save_note.json");
              impl "client.search_notes" ~raw:true
                ~conformance:(Some "vectors/search_notes.json");
            ];
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
    ("extension_model", extension_model);
    ("entries_client", entries_client);
    ("bespoke_impl", bespoke_impl);
  ]
