open Tono_frontend

(* ── Helpers ───────────────────────────────────────────────────────────── *)

let canon = Ir_json.to_canonical_string

let raises_invalid_ir f =
  try
    ignore (f ());
    false
  with Ir.Invalid_ir _ -> true

(* JSON-level round-trip: encode, decode, re-encode, and assert the canonical
   forms match. Since the encoder is injective on well-formed values, equal
   canonical encodings imply equal values, and this also pins the wire bytes. *)
let roundtrip name ~encode ~decode value =
  Alcotest.test_case name `Quick (fun () ->
      let j = encode value in
      match decode j with
      | Error e -> Alcotest.failf "decode failed: %s" e
      | Ok value' ->
          Alcotest.(check string) "round-trip" (canon j) (canon (encode value')))

let roundtrip_tref name t =
  roundtrip name ~encode:Ir_json.encode_tref ~decode:Ir_json.decode_tref t

let roundtrip_constraint name c =
  roundtrip name ~encode:Ir_json.encode_constraint
    ~decode:Ir_json.decode_constraint c

let roundtrip_trait name (value : Ir.json) =
  roundtrip name ~encode:Ir_json.encode_trait ~decode:Ir_json.decode_trait
    ({ trait_id = "core#trait"; value } : Ir.trait)

let roundtrip_shape name s =
  roundtrip name ~encode:Ir_json.encode_shape ~decode:Ir_json.decode_shape s

let roundtrip_member name m =
  roundtrip name ~encode:Ir_json.encode_member ~decode:Ir_json.decode_member m

let roundtrip_model name m =
  roundtrip name ~encode:Ir_json.encode_model ~decode:Ir_json.decode_model m

let decode_fails name ~decode j =
  Alcotest.test_case name `Quick (fun () ->
      match decode j with
      | Error _ -> ()
      | Ok _ -> Alcotest.fail "expected a decode error")

(* ── Smart constructors ────────────────────────────────────────────────── *)

let valid_bits () =
  List.iter
    (fun bits ->
      Alcotest.(check bool)
        (Printf.sprintf "i%d accepted" bits)
        false
        (raises_invalid_ir (fun () -> Ir.int_prim ~bits ~signed:true)))
    [ 8; 16; 32; 64 ]

let invalid_bits () =
  List.iter
    (fun bits ->
      Alcotest.(check bool)
        (Printf.sprintf "%d rejected" bits)
        true
        (raises_invalid_ir (fun () -> Ir.int_prim ~bits ~signed:false)))
    [ 0; 7; 33; 128 ]

let range_rejects_non_finite () =
  Alcotest.(check bool)
    "NaN min rejected" true
    (raises_invalid_ir (fun () -> Ir.range ~min:Float.nan ()));
  Alcotest.(check bool)
    "Inf max rejected" true
    (raises_invalid_ir (fun () -> Ir.range ~max:Float.infinity ()));
  Alcotest.(check bool)
    "finite range ok" false
    (raises_invalid_ir (fun () -> Ir.range ~min:0. ~max:10. ()))

let multiple_of_rejects_non_finite () =
  Alcotest.(check bool)
    "NaN rejected" true
    (raises_invalid_ir (fun () -> Ir.multiple_of Float.nan));
  Alcotest.(check bool)
    "Inf rejected" true
    (raises_invalid_ir (fun () -> Ir.multiple_of Float.neg_infinity));
  Alcotest.(check bool)
    "finite multipleOf ok" false
    (raises_invalid_ir (fun () -> Ir.multiple_of 2.5))

let union_default_discriminator () =
  match Ir.union ~params:[] ~members:[] () with
  | Ir.Union { discriminator; _ } ->
      Alcotest.(check string) "default discriminator" "type" discriminator
  | _ -> Alcotest.fail "expected a union"

let union_explicit_discriminator () =
  match Ir.union ~discriminator:"kind" ~params:[] ~members:[] () with
  | Ir.Union { discriminator; _ } ->
      Alcotest.(check string) "explicit discriminator" "kind" discriminator
  | _ -> Alcotest.fail "expected a union"

let constructor_suite =
  [
    Alcotest.test_case "valid int bits" `Quick valid_bits;
    Alcotest.test_case "invalid int bits" `Quick invalid_bits;
    Alcotest.test_case "range non-finite" `Quick range_rejects_non_finite;
    Alcotest.test_case "multipleOf non-finite" `Quick
      multiple_of_rejects_non_finite;
    Alcotest.test_case "union default discriminator" `Quick
      union_default_discriminator;
    Alcotest.test_case "union explicit discriminator" `Quick
      union_explicit_discriminator;
  ]

(* ── Index ─────────────────────────────────────────────────────────────── *)

let index_resolves () =
  let s : Ir.shape =
    {
      id = "payments#Charge";
      kind = Ir.Structure { params = []; members = [] };
      traits = [];
    }
  in
  let op : Ir.shape =
    {
      id = "payments#ListCharges";
      kind = Ir.Operation { input = None; output = None; errors = [] };
      traits = [];
    }
  in
  let m : Ir.model =
    {
      tono_ir_version = 1;
      modules =
        [ { mod_name = "payments"; shapes = [ s ]; operations = [ op ] } ];
    }
  in
  let idx = Ir.index_model m in
  Alcotest.(check bool)
    "shape found by id" true
    (Ir.Shape_map.mem "payments#Charge" idx.by_id);
  Alcotest.(check bool)
    "operation found by id" true
    (Ir.Shape_map.mem "payments#ListCharges" idx.by_id);
  Alcotest.(check bool)
    "unknown id absent" false
    (Ir.Shape_map.mem "payments#Nope" idx.by_id)

let index_suite = [ Alcotest.test_case "resolve by id" `Quick index_resolves ]

(* ── Primitives ────────────────────────────────────────────────────────── *)

let all_prims : (string * Ir.prim) list =
  [
    ("bool", Ir.Bool);
    ("string", Ir.String);
    ("bytes", Ir.Bytes);
    ("float", Ir.Float);
    ("timestamp", Ir.Timestamp);
    ("date", Ir.Date);
    ("duration", Ir.Duration);
    ("uuid", Ir.Uuid);
    ("i8", Ir.int_prim ~bits:8 ~signed:true);
    ("i16", Ir.int_prim ~bits:16 ~signed:true);
    ("i32", Ir.int_prim ~bits:32 ~signed:true);
    ("i64", Ir.int_prim ~bits:64 ~signed:true);
    ("u8", Ir.int_prim ~bits:8 ~signed:false);
    ("u16", Ir.int_prim ~bits:16 ~signed:false);
    ("u32", Ir.int_prim ~bits:32 ~signed:false);
    ("u64", Ir.int_prim ~bits:64 ~signed:false);
  ]

let prim_encodes_to_string () =
  List.iter
    (fun (s, p) ->
      Alcotest.(check string)
        (Printf.sprintf "%s encodes to its string" s)
        (Printf.sprintf "%S" s)
        (canon (Ir_json.encode_prim p)))
    all_prims

let prim_decodes_from_string () =
  List.iter
    (fun (s, p) ->
      match Ir_json.decode_prim (`String s) with
      | Ok p' ->
          Alcotest.(check string)
            (Printf.sprintf "%s decodes back" s)
            (canon (Ir_json.encode_prim p))
            (canon (Ir_json.encode_prim p'))
      | Error e -> Alcotest.failf "decode_prim %S failed: %s" s e)
    all_prims

let prim_suite =
  [
    Alcotest.test_case "encode to canonical string" `Quick
      prim_encodes_to_string;
    Alcotest.test_case "decode from canonical string" `Quick
      prim_decodes_from_string;
  ]

(* ── Type references ───────────────────────────────────────────────────── *)

let page_charge = Ir.Ref ("payments#Page", [ Ir.Ref ("payments#Charge", []) ])

let page_charge_exact_wire () =
  Alcotest.(check string)
    "Page[Charge] canonical wire"
    {|{"args":[{"args":[],"ref":"payments#Charge"}],"ref":"payments#Page"}|}
    (canon (Ir_json.encode_tref page_charge))

let tref_suite =
  [
    roundtrip_tref "prim" (Ir.Prim (Ir.int_prim ~bits:32 ~signed:true));
    roundtrip_tref "ref non-generic" (Ir.Ref ("payments#Charge", []));
    roundtrip_tref "ref generic" page_charge;
    roundtrip_tref "ref nested 2 levels"
      (Ir.Ref
         ( "core#Page",
           [ Ir.Ref ("core#List", [ Ir.Ref ("payments#Charge", []) ]) ] ));
    roundtrip_tref "param" (Ir.Param "T");
    roundtrip_tref "list" (Ir.List (Ir.Prim Ir.String));
    roundtrip_tref "map"
      (Ir.Map (Ir.Prim Ir.String, Ir.Ref ("payments#Charge", [])));
    Alcotest.test_case "Page[Charge] exact wire" `Quick page_charge_exact_wire;
  ]

(* ── Constraints ───────────────────────────────────────────────────────── *)

let custom_in_constraints_rejected () =
  let m : Ir.member =
    {
      name = "card";
      target = Ir.Prim Ir.String;
      required = true;
      default = None;
      constraints =
        [ Ir.Custom { name = "luhn"; binding = [ ("ts", "x#luhn") ] } ];
      traits = [];
    }
  in
  Alcotest.(check bool)
    "custom in constraints rejected by encoder" true
    (raises_invalid_ir (fun () -> Ir_json.encode_member m))

let custom_as_trait_roundtrips () =
  let m : Ir.member =
    {
      name = "card";
      target = Ir.Prim Ir.String;
      required = true;
      default = None;
      constraints = [];
      traits =
        [ { trait_id = "x#luhn"; value = `Assoc [ ("kind", `String "luhn") ] } ];
    }
  in
  let j = Ir_json.encode_member m in
  match Ir_json.decode_member j with
  | Error e -> Alcotest.failf "decode failed: %s" e
  | Ok m' ->
      Alcotest.(check string)
        "custom constraint as trait round-trips" (canon j)
        (canon (Ir_json.encode_member m'));
      Alcotest.(check int) "no core constraints" 0 (List.length m'.constraints);
      Alcotest.(check int) "one trait" 1 (List.length m'.traits)

let constraint_suite =
  [
    roundtrip_constraint "range full"
      (Ir.range ~min:0. ~max:100. ~excl_min:true ~excl_max:false ());
    roundtrip_constraint "range min only" (Ir.range ~min:1. ());
    roundtrip_constraint "range max only" (Ir.range ~max:9. ());
    roundtrip_constraint "range empty bounds" (Ir.range ());
    roundtrip_constraint "length full" (Ir.length ~min:1 ~max:255 ());
    roundtrip_constraint "length min only" (Ir.length ~min:1 ());
    roundtrip_constraint "pattern" (Ir.pattern "^[a-z]+$");
    roundtrip_constraint "multipleOf" (Ir.multiple_of 0.25);
    Alcotest.test_case "custom rejected in constraints" `Quick
      custom_in_constraints_rejected;
    Alcotest.test_case "custom as trait round-trips" `Quick
      custom_as_trait_roundtrips;
  ]

(* ── Members ───────────────────────────────────────────────────────────── *)

let full_member : Ir.member =
  {
    name = "amount";
    target = Ir.Prim (Ir.int_prim ~bits:64 ~signed:false);
    required = true;
    default = Some (`Int 0);
    constraints = [ Ir.range ~min:0. () ];
    traits = [ { trait_id = "core#wire"; value = `String "amt" } ];
  }

let default_none_omits_key () =
  let m = { full_member with default = None } in
  match Ir_json.encode_member m with
  | `Assoc kvs ->
      Alcotest.(check bool)
        "no default key" false
        (List.mem_assoc "default" kvs)
  | _ -> Alcotest.fail "expected an object"

let default_some_emits_key () =
  match Ir_json.encode_member full_member with
  | `Assoc kvs ->
      Alcotest.(check bool)
        "default key present" true
        (List.mem_assoc "default" kvs)
  | _ -> Alcotest.fail "expected an object"

let member_suite =
  [
    roundtrip_member "full member" full_member;
    roundtrip_member "nullable member" { full_member with required = false };
    roundtrip_member "default null" { full_member with default = Some `Null };
    roundtrip_member "required with default" full_member;
    roundtrip_member "optional with default"
      { full_member with required = false; default = Some (`Int 7) };
    Alcotest.test_case "default None omits key" `Quick default_none_omits_key;
    Alcotest.test_case "default Some emits key" `Quick default_some_emits_key;
  ]

(* ── Traits ────────────────────────────────────────────────────────────── *)

let trait_suite =
  [
    roundtrip_trait "null value" `Null;
    roundtrip_trait "bool value" (`Bool true);
    roundtrip_trait "int value" (`Int 42);
    roundtrip_trait "big int value" (`Intlit "99999999999999999999");
    roundtrip_trait "float value" (`Float 1.5);
    roundtrip_trait "string value" (`String "hello");
    roundtrip_trait "array value" (`List [ `Int 1; `String "a"; `Null ]);
    roundtrip_trait "object value"
      (`Assoc [ ("z", `Bool false); ("a", `List []) ]);
  ]

(* ── Shapes ────────────────────────────────────────────────────────────── *)

let struct_shape : Ir.shape =
  {
    id = "payments#Charge";
    kind =
      Ir.Structure
        {
          params = [];
          members =
            [
              {
                name = "id";
                target = Ir.Prim Ir.Uuid;
                required = true;
                default = None;
                constraints = [];
                traits = [];
              };
              {
                name = "note";
                target = Ir.Prim Ir.String;
                required = false;
                default = None;
                constraints = [ Ir.length ~max:280 () ];
                traits = [];
              };
            ];
        };
    traits = [ { trait_id = "core#doc"; value = `String "A charge." } ];
  }

let generic_struct : Ir.shape =
  {
    id = "core#Page";
    kind =
      Ir.Structure
        {
          params = [ "T" ];
          members =
            [
              {
                name = "items";
                target = Ir.List (Ir.Param "T");
                required = true;
                default = None;
                constraints = [];
                traits = [];
              };
            ];
        };
    traits = [];
  }

let union_shape : Ir.shape =
  {
    id = "payments#Source";
    kind =
      Ir.union ~params:[]
        ~members:
          [
            {
              name = "card";
              target = Ir.Ref ("payments#Card", []);
              required = true;
              default = None;
              constraints = [];
              traits = [];
            };
            {
              name = "bank";
              target = Ir.Ref ("payments#Bank", []);
              required = true;
              default = None;
              constraints = [];
              traits =
                [ { trait_id = "core#wire"; value = `String "bank_account" } ];
            };
          ]
        ();
    traits = [];
  }

let enum_shape : Ir.shape =
  {
    id = "payments#Status";
    kind =
      Ir.Enum
        {
          backing = `String;
          values = [ ("active", None); ("closed", None) ];
          open_ = true;
        };
    traits = [];
  }

let int_enum_shape : Ir.shape =
  {
    id = "payments#Code";
    kind =
      Ir.Enum
        {
          backing = `Int;
          values = [ ("ok", Some 0); ("fail", Some 1) ];
          open_ = false;
        };
    traits = [];
  }

let service_shape : Ir.shape =
  {
    id = "payments#Payments";
    kind = Ir.Service { operations = [ "payments#ListCharges" ] };
    traits = [];
  }

let operation_shape : Ir.shape =
  {
    id = "payments#ListCharges";
    kind =
      Ir.Operation
        {
          input = None;
          output = Some page_charge;
          errors = [ Ir.Ref ("payments#NotFound", []) ];
        };
    traits = [];
  }

let union_keeps_discriminator () =
  match Ir_json.encode_shape union_shape with
  | `Assoc kvs ->
      Alcotest.(check bool)
        "discriminator present" true
        (List.mem_assoc "discriminator" kvs)
  | _ -> Alcotest.fail "expected an object"

let operation_nulls_absent_io () =
  match Ir_json.encode_shape operation_shape with
  | `Assoc kvs -> (
      match List.assoc_opt "input" kvs with
      | Some `Null -> ()
      | _ -> Alcotest.fail "absent input must encode as null")
  | _ -> Alcotest.fail "expected an object"

let shape_suite =
  [
    roundtrip_shape "structure" struct_shape;
    roundtrip_shape "generic structure" generic_struct;
    roundtrip_shape "union" union_shape;
    roundtrip_shape "string enum (open)" enum_shape;
    roundtrip_shape "int enum (closed)" int_enum_shape;
    roundtrip_shape "service" service_shape;
    roundtrip_shape "operation" operation_shape;
    Alcotest.test_case "union keeps discriminator" `Quick
      union_keeps_discriminator;
    Alcotest.test_case "operation nulls absent io" `Quick
      operation_nulls_absent_io;
  ]

(* ── Model / envelope ──────────────────────────────────────────────────── *)

let sample_model : Ir.model =
  {
    tono_ir_version = Ir_json.current_ir_version;
    modules =
      [
        {
          mod_name = "payments";
          shapes =
            [
              struct_shape;
              generic_struct;
              union_shape;
              enum_shape;
              service_shape;
            ];
          operations = [ operation_shape ];
        };
      ];
  }

let model_suite = [ roundtrip_model "sample model" sample_model ]

(* ── Negative decode ───────────────────────────────────────────────────── *)

let negative_suite =
  [
    decode_fails "unknown primitive" ~decode:Ir_json.decode_prim
      (`String "varchar");
    decode_fails "decimal primitive rejected" ~decode:Ir_json.decode_prim
      (`String "decimal");
    decode_fails "out-of-range int width" ~decode:Ir_json.decode_prim
      (`String "i7");
    decode_fails "prim not a string" ~decode:Ir_json.decode_prim (`Int 5);
    decode_fails "tref zero variant keys" ~decode:Ir_json.decode_tref
      (`Assoc [ ("nope", `Int 1) ]);
    decode_fails "tref multiple variant keys" ~decode:Ir_json.decode_tref
      (`Assoc
         [
           ("prim", `String "i32"); ("list", `Assoc [ ("prim", `String "bool") ]);
         ]);
    decode_fails "tref ref missing args" ~decode:Ir_json.decode_tref
      (`Assoc [ ("ref", `String "x#Y") ]);
    decode_fails "tref prim with extra key" ~decode:Ir_json.decode_tref
      (`Assoc [ ("prim", `String "i32"); ("extra", `Int 1) ]);
    decode_fails "tref decimal inside prim" ~decode:Ir_json.decode_tref
      (`Assoc [ ("prim", `String "decimal") ]);
    decode_fails "constraint no key" ~decode:Ir_json.decode_constraint
      (`Assoc []);
    decode_fails "constraint multiple keys" ~decode:Ir_json.decode_constraint
      (`Assoc [ ("pattern", `String "x"); ("multipleOf", `Float 1.) ]);
    decode_fails "model wrong version" ~decode:Ir_json.decode_model
      (`Assoc [ ("tono_ir_version", `Int 2); ("modules", `List []) ]);
    decode_fails "model missing version" ~decode:Ir_json.decode_model
      (`Assoc [ ("modules", `List []) ]);
  ]

let version_gate_accepts_current () =
  match
    Ir_json.decode_model
      (`Assoc
         [
           ("tono_ir_version", `Int Ir_json.current_ir_version);
           ("modules", `List []);
         ])
  with
  | Ok _ -> ()
  | Error e -> Alcotest.failf "current version should decode: %s" e

let version_suite =
  [
    Alcotest.test_case "current version accepted" `Quick
      version_gate_accepts_current;
  ]

(* ── Runner ────────────────────────────────────────────────────────────── *)

let () =
  Alcotest.run "ir"
    [
      ("constructors", constructor_suite);
      ("index", index_suite);
      ("prim", prim_suite);
      ("tref", tref_suite);
      ("constraint", constraint_suite);
      ("member", member_suite);
      ("trait", trait_suite);
      ("shape", shape_suite);
      ("model", model_suite);
      ("version", version_suite);
      ("negative", negative_suite);
    ]
