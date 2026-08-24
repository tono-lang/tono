(* JSON wire encoding for the IR. This is the contract the Rust backend mirrors.
   Rules:
   - primitives are bare strings ("i32", "string", ...);
   - a [tref] is a single-key tagged object, except [ref] which carries a sibling
     "args" array ({"ref": <id>, "args": [...]});
   - a core constraint is a single-key tagged object with camelCase fields;
   - a [shape] is internally tagged by a "kind" field flattened next to "id" and
     "traits";
   - the envelope carries a bare integer "tono_ir_version" gate.
   Encoders assume well-formed in-memory values and raise [Ir.Invalid_ir] on the
   few things JSON cannot represent. Decoders take untrusted input and return a
   [result]. *)

(* The IR schema revision this build understands. Bumped by one on every
   incompatible change to the wire format; there is no negotiation.
   v2 removed the enum [open] field (every enum is open).
   v3 added the module [extensions] table (bespoke hooks/contracts/constraints).
   v4 made an enum value an object ({"name", "value"?, "traits"}) so it can carry
   a trait bag (documentation rides it), replacing the [name, intOrNull] pair.
   v5 added the entry model: "entry"/"config" shape kinds whose fields carry
   value sources, @format templates, transform pipelines, match selection, and
   @bind composition; entry ops nest inside the entry shape and their trait
   values may carry field references ({"field": [...]}).
   v6 added the "impl" extension kind (a bespoke operation implementation) and
   its optional "raw" flag.
   v7 added the module [tests] table (declared tests: constructions, stubs,
   calls, and expect patterns, with values in wire form).
   v8 added an operation's optional "wire" field: the resolved HTTP binding
   (method, uri, per-member bindings, response bindings, success status
   codes, and the entry-scoped endpoint/header/timeout/retry refs) as typed
   structure, replacing the opaque wire_descriptor trait blob for direct
   consumption (the blob itself still rode the trait bag, unchanged, for
   backward compatibility).
   v9 removed the wire_descriptor trait blob the frontend attached alongside
   the typed "wire" field for backward compatibility; "wire" is the only
   representation now.
   v10 gave @errorCode a mandatory first argument: the dotted path to probe in
   the error body, ahead of the discriminator value. The trait's IR value is
   still a plain array of trait args, so the shape is unchanged; only its
   arity is.
   v11 changed the meaning of the wire binding's "success": empty now means
   no @http(code:) was declared (the 2xx-range convention applies), where
   every prior version always populated it with at least the default 200.
   v12 named the op parameter ("input_name", absent for the legacy unnamed
   form) and unified the request-value grammar: "uri"/"endpoint" became a
   wire_value (literal, entry/param reference, or template) instead of a bare
   template/path, template_part/wire_value gained a "param" variant for a
   reference into the op's own parameter, and wire_binding gained "query"
   (mirrors "request_headers") for the new op-level @query trait.
   v13 removed the per-member HTTP binding traits (httpLabel/httpQuery/
   httpHeader/httpPayload) and the "bindings" map they resolved into:
   wire_binding gained "body" (a wire_value, absent when the operation sends
   none), and wire_value gained an "object" variant (named fields, each a
   wire_value) for the new @body ctor-mapper form.
   v14 added the module "ext_libs" table: per-language module paths, foreign
   struct/opaque-handle declarations, and "extern" declarations with a
   per-language call/yields/returns/errors binding (surface-and-IR only; no
   typecheck or codegen consumes it yet). An entry field's "call" key
   (alongside the existing "select") carries a field's [= ns.fn(args)]
   extern-call source, and a call argument gained "lit"/"list"/"call"
   variants for ctor-field values.
   v15 added an operation's optional "impl_call": a third implementation
   source (the op's own "impl .field.method(args)" body), alongside a
   protocol's "wire" and a legacy "ext impl" extension. It reuses the "lit"
   call-argument variant v14 introduced for ctor-field values, now also
   reachable as a bare call argument (e.g. the "notes" in
   [.bus.send("notes", .payload.body)]) -- no wire-shape change there, only
   a new use site. Surface-and-IR only; no codegen consumes "impl_call" yet.
   v17 added wire_value's "call" variant: an extern call read as a
   @header/@query/@body value. Its own arguments mirror an ordinary call
   argument's "field"/"param"/"lit"/"ctor" tags, plus the bare string
   "request" for ".request", the canonical already-assembled request --
   legal only in this position, never during entry construction (that side
   is rejected at typecheck).
   v18 added a test's "extern_stubs" table: a declared test can now stub an
   "ext"/"extern" FFI free function or opaque-handle method, alongside the
   existing client/op "stubs" table.
   v19 dropped the "hook" extension kind: the lifecycle it filled is gone,
   replaced entirely by the declarative "ext"/"extern" FFI model v14-v18
   added; a hook is now rejected at typecheck and never reaches the IR.
   v21 added an opaque type's optional "instance": which instantiation of a
   foreign generic type the handle names (the foreign type's own name, plus
   the tono argument it is monomorphized with). Absent for a foreign type
   that is not generic.
   v22 added an entry field's optional "handle_call" (alongside "call" and
   "select"): a field whose value source is a call into a sibling opaque
   handle's method ([config: cfg = .provider.get()]), so one foreign
   resolution feeds several operations. It reuses v15's "impl_call" shape
   ("recv"/"method"/"args") in the field position. *)
(* v25 widened call:'s argument shapes: call_arg gained Ca_symbol_call (a
   bare foreign-symbol call nested inside a call: line's own argument list,
   legal at the top level unlike Ca_call), extern_param gained xp_variadic,
   and extern_lang gained el_new. *)
(* v26 added extern_lang's optional el_receiver: the call: line's own
   receiver when it is a foreign type name (a static method, Type.method),
   not a value. Absent for a free function of the library. *)
(* v27 widened call_arg with Ca_type: a declared opaque handle passed as a
   class reference (the library constructs it), carried as the handle's
   tono name and spelled by each emitter as that handle's foreign name. *)
(* v29 added an operation's "output_nullable" flag (omitted when false): a
   declared [T?] return, mirroring the member-level "required" flag since
   nullability is not a type node. *)
(* v30 added a call ctor's optional "as": the struct literal under a foreign
   spelling of its own ([opts { .. }: #(&Options)]), the same annotation a
   parameter carries, for a library that takes the form by pointer. Absent
   for a literal passed as the form's own type. *)
let current_ir_version = 31

(* The scalar and entry-model codecs live in [Ir_json_base] and
   [Ir_json_entry]; re-exported here so [Ir_json] stays the single entry
   point the tools and the backend contract reference. *)
let encode_prim = Ir_json_base.encode_prim
let decode_prim = Ir_json_base.decode_prim
let encode_tref = Ir_json_base.encode_tref
let decode_tref = Ir_json_base.decode_tref
let decode_tref_opt = Ir_json_base.decode_tref_opt
let encode_constraint = Ir_json_base.encode_constraint
let decode_constraint = Ir_json_base.decode_constraint
let encode_trait = Ir_json_base.encode_trait
let decode_trait = Ir_json_base.decode_trait
let encode_member = Ir_json_base.encode_member
let decode_member = Ir_json_base.decode_member
let encode_enum_value = Ir_json_base.encode_enum_value
let decode_enum_value = Ir_json_base.decode_enum_value
let encode_backing = Ir_json_base.encode_backing
let encode_source = Ir_json_entry.encode_source
let decode_source = Ir_json_entry.decode_source
let encode_template_part = Ir_json_entry.encode_template_part
let decode_template_part = Ir_json_entry.decode_template_part
let encode_arm_value = Ir_json_entry.encode_arm_value
let decode_arm_value = Ir_json_entry.decode_arm_value
let encode_select = Ir_json_entry.encode_select
let decode_select = Ir_json_entry.decode_select
let encode_bind = Ir_json_entry.encode_bind
let decode_bind = Ir_json_entry.decode_bind
let encode_entry_field = Ir_json_entry.encode_entry_field
let decode_entry_field = Ir_json_entry.decode_entry_field
let encode_ext_lib = Ir_json_extern.encode_ext_lib
let decode_ext_lib = Ir_json_extern.decode_ext_lib
let encode_test = Ir_json_tests.encode_test
let decode_test = Ir_json_tests.decode_test
let encode_wire_response_part = Ir_json_wire.encode_wire_response_part
let decode_wire_response_part = Ir_json_wire.decode_wire_response_part
let encode_wire_value = Ir_json_wire.encode_wire_value
let decode_wire_value = Ir_json_wire.decode_wire_value
let encode_wire_binding = Ir_json_wire.encode_wire_binding
let decode_wire_binding = Ir_json_wire.decode_wire_binding
let decode_wire_binding_opt = Ir_json_wire.decode_wire_binding_opt
let ( let* ) = Result.bind
let err fmt = Printf.ksprintf (fun s -> Error s) fmt
let map_result = Ir_json_base.map_result
let as_assoc = Ir_json_base.as_assoc
let as_list = Ir_json_base.as_list
let as_string = Ir_json_base.as_string
let as_int = Ir_json_base.as_int

(* The handle-method-call codec lives in [Ir_json_entry] (an entry field's
   own "handle_call" source shares it); aliased for the operation shape. *)
let encode_op_impl_call = Ir_json_entry.encode_op_impl_call
let decode_op_impl_call = Ir_json_entry.decode_op_impl_call

(* ── Shapes, modules, and the model envelope ───────────────────────────── *)

let rec encode_shape_kind_fields (k : Ir.shape_kind) : (string * Ir.json) list =
  let params ps = `List (List.map (fun p -> `String p) ps) in
  let members ms = `List (List.map encode_member ms) in
  match k with
  | Structure { params = ps; members = ms } ->
      [
        ("kind", `String "structure");
        ("params", params ps);
        ("members", members ms);
      ]
  | Union { params = ps; members = ms; discriminator } ->
      [
        ("kind", `String "union");
        ("params", params ps);
        ("members", members ms);
        ("discriminator", `String discriminator);
      ]
  | Enum { backing; values } ->
      [
        ("kind", `String "enum");
        ("backing", `String (encode_backing backing));
        ("values", `List (List.map encode_enum_value values));
      ]
  | Service { operations } ->
      [
        ("kind", `String "service");
        ("operations", `List (List.map (fun s -> `String s) operations));
      ]
  | Operation
      { input; input_name; output; output_nullable; errors; wire; impl_call }
    -> (
      let opt = function None -> `Null | Some t -> encode_tref t in
      [
        ("kind", `String "operation");
        ("input", opt input);
        ("output", opt output);
        ("errors", `List (List.map encode_tref errors));
      ]
      (* Omitted when false, like the extension [raw] flag: the non-nullable
         return is the default. *)
      @ (if output_nullable then [ ("output_nullable", `Bool true) ] else [])
      @ (match input_name with
        | None -> []
        | Some n -> [ ("input_name", `String n) ])
      @ (match wire with
        | None -> []
        | Some w -> [ ("wire", encode_wire_binding w) ])
      @
      match impl_call with
      | None -> []
      | Some c -> [ ("impl_call", encode_op_impl_call c) ])
  | Entry { fields; operations } ->
      [
        ("kind", `String "entry");
        ("fields", `List (List.map encode_entry_field fields));
        ("operations", `List (List.map encode_shape operations));
      ]
  | Config { fields } ->
      [
        ("kind", `String "config");
        ("fields", `List (List.map encode_entry_field fields));
      ]

and encode_shape (s : Ir.shape) : Ir.json =
  `Assoc
    ((("id", `String s.id) :: encode_shape_kind_fields s.kind)
    @ [ ("traits", `List (List.map encode_trait s.traits)) ])

let encode_ext_kind = function
  | Ir.Contract -> "contract"
  | Ir.Constraint -> "constraint"
  | Ir.Impl -> "impl"

let encode_ext_sig (s : Ir.ext_sig) : Ir.json =
  `Assoc [ ("input", encode_tref s.input); ("output", encode_tref s.output) ]

let encode_binding (b : Ir.binding) : Ir.json =
  `Assoc (List.map (fun (lang, target) -> (lang, `String target)) b)

let encode_extension (e : Ir.extension) : Ir.json =
  `Assoc
    ([
       ("name", `String e.ext_name);
       ("kind", `String (encode_ext_kind e.ext_kind));
     ]
    @ (match e.ext_sig with
      | None -> []
      | Some s -> [ ("signature", encode_ext_sig s) ])
    (* The typed form is the default, so "raw" rides the wire only when set. *)
    @ (if e.ext_raw then [ ("raw", `Bool true) ] else [])
    @ [ ("bindings", encode_binding e.ext_bindings) ]
    @
    match e.ext_conformance with
    | None -> []
    | Some c -> [ ("conformance", `String c) ])

let encode_module (m : Ir.module_) : Ir.json =
  `Assoc
    [
      ("name", `String m.mod_name);
      ("shapes", `List (List.map encode_shape m.shapes));
      ("operations", `List (List.map encode_shape m.operations));
      ("extensions", `List (List.map encode_extension m.extensions));
      ("ext_libs", `List (List.map encode_ext_lib m.ext_libs));
      ("tests", `List (List.map encode_test m.tests));
    ]

let encode_model (m : Ir.model) : Ir.json =
  `Assoc
    [
      ("tono_ir_version", `Int m.tono_ir_version);
      ("modules", `List (List.map encode_module m.modules));
    ]

let rec decode_shape_kind kvs =
  let get k = List.assoc_opt k kvs in
  let* kind =
    match get "kind" with
    | Some v -> as_string v
    | None -> err "shape is missing kind"
  in
  let params () =
    match get "params" with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result as_string xs
  in
  let members () =
    match get "members" with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_member xs
  in
  match kind with
  | "structure" ->
      let* params = params () in
      let* members = members () in
      Ok (Ir.Structure { params; members })
  | "union" ->
      let* params = params () in
      let* members = members () in
      let* discriminator =
        match get "discriminator" with
        | None -> Ok "type"
        | Some (`String s) -> Ok s
        | Some _ -> err "union discriminator must be a string"
      in
      Ok (Ir.Union { params; members; discriminator })
  | "enum" ->
      let* backing =
        match get "backing" with
        | Some (`String "string") -> Ok `String
        | Some (`String "int") -> Ok `Int
        | Some _ -> err "enum backing must be \"string\" or \"int\""
        | None -> err "enum is missing backing"
      in
      let* values =
        match get "values" with
        | None -> Ok []
        | Some v ->
            let* xs = as_list v in
            map_result decode_enum_value xs
      in
      Ok (Ir.Enum { backing; values })
  | "service" ->
      let* operations =
        match get "operations" with
        | None -> Ok []
        | Some v ->
            let* xs = as_list v in
            map_result as_string xs
      in
      Ok (Ir.Service { operations })
  | "operation" ->
      let* input = decode_tref_opt (get "input") in
      let* input_name =
        match get "input_name" with
        | None -> Ok None
        | Some v ->
            let* s = as_string v in
            Ok (Some s)
      in
      let* output = decode_tref_opt (get "output") in
      let* output_nullable =
        match get "output_nullable" with
        | None -> Ok false
        | Some v -> Ir_json_base.as_bool v
      in
      let* errors =
        match get "errors" with
        | None -> Ok []
        | Some v ->
            let* xs = as_list v in
            map_result decode_tref xs
      in
      let* wire = decode_wire_binding_opt (get "wire") in
      let* impl_call =
        match get "impl_call" with
        | None -> Ok None
        | Some v ->
            let* c = decode_op_impl_call v in
            Ok (Some c)
      in
      Ok
        (Ir.Operation
           {
             input;
             input_name;
             output;
             output_nullable;
             errors;
             wire;
             impl_call;
           })
  | "entry" ->
      let* fields = decode_fields (get "fields") in
      let* operations =
        match get "operations" with
        | None -> Ok []
        | Some v ->
            let* xs = as_list v in
            map_result decode_shape xs
      in
      Ok (Ir.Entry { fields; operations })
  | "config" ->
      let* fields = decode_fields (get "fields") in
      Ok (Ir.Config { fields })
  | other -> err "unknown shape kind %S" other

and decode_fields = function
  | None -> Ok []
  | Some v ->
      let* xs = as_list v in
      map_result decode_entry_field xs

and decode_shape j =
  let* kvs = as_assoc j in
  let* id =
    match List.assoc_opt "id" kvs with
    | Some v -> as_string v
    | None -> err "shape is missing id"
  in
  let* kind = decode_shape_kind kvs in
  let* traits =
    match List.assoc_opt "traits" kvs with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_trait xs
  in
  Ok ({ id; kind; traits } : Ir.shape)

let decode_ext_kind j =
  let* s = as_string j in
  match s with
  | "contract" -> Ok Ir.Contract
  | "constraint" -> Ok Ir.Constraint
  | "impl" -> Ok Ir.Impl
  | other -> err "unknown extension kind %S" other

let decode_ext_sig j =
  let* kvs = as_assoc j in
  let field k =
    match List.assoc_opt k kvs with
    | Some v -> decode_tref v
    | None -> err "extension signature is missing %s" k
  in
  let* input = field "input" in
  let* output = field "output" in
  Ok ({ input; output } : Ir.ext_sig)

let decode_binding j =
  let* kvs = as_assoc j in
  map_result
    (fun (lang, v) ->
      let* target = as_string v in
      Ok (lang, target))
    kvs

let decode_extension j =
  let* kvs = as_assoc j in
  let get k = List.assoc_opt k kvs in
  let* ext_name =
    match get "name" with
    | Some v -> as_string v
    | None -> err "extension is missing name"
  in
  let* ext_kind =
    match get "kind" with
    | Some v -> decode_ext_kind v
    | None -> err "extension is missing kind"
  in
  let* ext_sig =
    match get "signature" with
    | None -> Ok None
    | Some v ->
        let* s = decode_ext_sig v in
        Ok (Some s)
  in
  let* ext_raw =
    match get "raw" with None -> Ok false | Some v -> Ir_json_base.as_bool v
  in
  let* ext_bindings =
    match get "bindings" with None -> Ok [] | Some v -> decode_binding v
  in
  let* ext_conformance =
    match get "conformance" with
    | None -> Ok None
    | Some v ->
        let* s = as_string v in
        Ok (Some s)
  in
  Ok
    ({ ext_name; ext_kind; ext_sig; ext_raw; ext_bindings; ext_conformance }
      : Ir.extension)

let decode_module j =
  let* kvs = as_assoc j in
  let* mod_name =
    match List.assoc_opt "name" kvs with
    | Some v -> as_string v
    | None -> err "module is missing name"
  in
  let shapes_of k =
    match List.assoc_opt k kvs with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_shape xs
  in
  let* shapes = shapes_of "shapes" in
  let* operations = shapes_of "operations" in
  let* extensions =
    match List.assoc_opt "extensions" kvs with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_extension xs
  in
  let* ext_libs =
    match List.assoc_opt "ext_libs" kvs with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_ext_lib xs
  in
  let* tests =
    match List.assoc_opt "tests" kvs with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_test xs
  in
  Ok
    ({ mod_name; shapes; operations; extensions; ext_libs; tests } : Ir.module_)

let decode_model j =
  let* kvs = as_assoc j in
  let* version =
    match List.assoc_opt "tono_ir_version" kvs with
    | Some v -> as_int v
    | None -> err "model is missing tono_ir_version"
  in
  if version <> current_ir_version then
    err "unsupported tono_ir_version %d (this build supports %d)" version
      current_ir_version
  else
    let* modules =
      match List.assoc_opt "modules" kvs with
      | None -> Ok []
      | Some v ->
          let* xs = as_list v in
          map_result decode_module xs
    in
    Ok ({ tono_ir_version = version; modules } : Ir.model)

(* ── Canonical form (for stable comparison across emitters) ────────────── *)

(* Recursively sorts object keys and collapses [`Intlit] that fits a native int.
   Used to compare JSON produced by different emitters (yojson, serde_json)
   without depending on key order or whitespace. *)
let rec canonicalize (j : Ir.json) : Ir.json =
  match j with
  | `Assoc kvs ->
      `Assoc
        (List.sort
           (fun (a, _) (b, _) -> String.compare a b)
           (List.map (fun (k, v) -> (k, canonicalize v)) kvs))
  | `List xs -> `List (List.map canonicalize xs)
  | `Intlit s -> (
      match int_of_string_opt s with Some i -> `Int i | None -> `Intlit s)
  | (`Null | `Bool _ | `Int _ | `Float _ | `String _) as leaf -> leaf

let to_canonical_string (j : Ir.json) : string =
  Yojson.Safe.to_string (canonicalize j)

(* Re-exported low-level helpers so their edge cases can be tested directly while
   staying out of the intended public surface (see the .mli). *)
module Internal = struct
  let as_int = Ir_json_base.as_int
  let as_float = Ir_json_base.as_float
end
