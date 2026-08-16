open Tono_frontend

(* Positions where entry-model constructions used to be accepted and then
   dropped in silence: wire members carrying selection/derivation metadata,
   loose ops with entry-only protocol traits, templates inside @env names,
   non-string header values, and duplicated non-repeatable traits (the
   trailing-trait absorption footgun now yields a diagnostic). *)

let check src =
  let file, _ = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let _, tc = Typecheck.check_module ~file m in
  tc

let codes src = List.filter_map (fun (d : Diagnostic.t) -> d.code) (check src)

let contains hay needle =
  let hn = String.length hay and nn = String.length needle in
  let rec loop i =
    if i + nn > hn then false
    else if String.sub hay i nn = needle then true
    else loop (i + 1)
  in
  nn = 0 || loop 0

let expect what wanted src =
  Alcotest.(check (list string)) what wanted (codes src)

let wire = "struct r { y: string }\n"

let entry fields =
  "pub struct c {\n" ^ fields
  ^ "\n\
    \  ep: string @env(\"EP\")\n\
    \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep)\n\
     }\n" ^ wire

(* ── Silent-drop positions get diagnostics ─────────────────────────────── *)

let wire_member_match_rejected () =
  expect "match on a wire member" [ "TC0035" ]
    "struct s { v: string, e: string = match .v { _ => \"x\" } }"

let wire_member_format_rejected () =
  expect "format on a wire member" [ "TC0035" ]
    "struct s { a: string @format(\"{.b}\"), b: string }"

let wire_member_transform_rejected () =
  expect "transform on a wire member" [ "TC0035" ]
    "struct s { a: string @str::trim }"

let wire_member_bind_rejected () =
  expect "bind on a wire member" [ "TC0042" ]
    "struct s { a: string @bind(x, .a) }"

let loose_op_literal_timeout_rejected () =
  expect "literal timeout on a loose op" [ "TC0044" ]
    "struct w { x: string }\n\
     op o(w): w @http(method: \"GET\", path: \"/\") @timeout(5)"

let loose_op_literal_retry_rejected () =
  expect "literal retry on a loose op" [ "TC0044" ]
    "struct w { x: string }\n\
     op o(w): w @http(method: \"GET\", path: \"/\") @retry(3)"

let env_with_placeholder_rejected () =
  expect "template inside @env" [ "TC0035" ]
    (entry "  k: string @env(\"ENDPOINT_{.ep}\")")

let header_literal_int_value_rejected () =
  expect "int header value" [ "TC0044" ]
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep) \
      @header(\"K\", 5)\n\
      }\n" ^ wire)

let absorbed_doc_duplicate_rejected () =
  (* The known footgun: a @doc on its own line between an op and the next
     field binds to the op; with a @doc already there, the duplicate now
     yields a diagnostic instead of silently doubling. *)
  expect "absorbed doc duplicates" [ "TC0047" ]
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep) \
      @doc(\"api\")\n\
     \  @doc(\"meant for the field below\")\n\
     \  k: string @env(\"K\")\n\
      }\n" ^ wire)

let duplicate_doc_on_decl_rejected () =
  expect "duplicate decl doc" [ "TC0047" ]
    "@doc(\"a\")\n@doc(\"b\")\nstruct s { x: string }"

let repeatable_traits_stay_legal () =
  expect "repeated env and errors stay legal" []
    ("@status(500) @errorCode(\"code\", \"a\") struct e1 { m: string }\n\
      @status(501) @errorCode(\"code\", \"b\") struct e2 { m: string }\n"
    ^ entry
        "  k: string @env(\"A\") @env(\"B\") @default(\"x\")\n\
        \  op p(): r @http(method: \"GET\", path: \"/p\", endpoint: .ep) \
         @errors(e1, e2) @header(\"A\", .k) @header(\"B\", .k)")

let config_boundary_names_composition_point () =
  let diags =
    check
      ("struct conf { api_key: string }\n"
      ^ entry "  settings: conf @bind(api_key, .ep)"
      ^ "op outer(conf): r")
  in
  Alcotest.(check (list string))
    "one boundary error" [ "TC0034" ]
    (List.filter_map (fun (d : Diagnostic.t) -> d.code) diags);
  Alcotest.(check bool)
    "message cites the composition point" true
    (contains (List.hd diags).message
       "composes it via @bind on field 'settings'")

(* ── Typed refs and remaining review paths ─────────────────────────────── *)

let bind_type_mismatch_rejected () =
  expect "i32 bound into a string field" [ "TC0042" ]
    ("struct conf { api_key: string @env(\"K\") }\n"
   ^ "pub struct c {\n\
     \  max_retries: i32 @with @default(3)\n\
     \  s: conf @bind(api_key, .max_retries)\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep)\n\
      }\n" ^ wire)

let env_ref_must_be_string () =
  expect "i32 naming an env variable" [ "TC0035" ]
    (entry "  n: i32 @with @default(1)\n  k: string @env(.n)")

let duplicate_entry_op_rejected () =
  expect "two ops with one name" [ "TC0002" ]
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: \"/a\", endpoint: .ep)\n\
     \  op o(): r @http(method: \"GET\", path: \"/b\", endpoint: .ep)\n\
      }\n" ^ wire)

(* The unified request-value grammar accepts a template in a header value
   (key and value share one grammar). *)
let header_value_template_accepted () =
  expect "template in a header value" []
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep) \
      @header(\"K\", \"{.ep}-x\")\n\
      }\n" ^ wire)

let header_key_input_placeholder_rejected () =
  expect "input placeholder in a header key" [ "TC0044" ]
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep) \
      @header(\"X-{id}\", .ep)\n\
      }\n" ^ wire)

let header_key_crlf_rejected () =
  expect "carriage return/line feed in a header key" [ "TC0044" ]
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep) \
      @header(\"X\\r\\nEvil\", .ep)\n\
      }\n" ^ wire)

(* A map or list value (nullable or not) has no defined header/query
   serialization. *)
let header_nullable_list_value_rejected () =
  let src =
    "pub struct c {\n\
    \  ep: string @env(\"EP\")\n\
    \  tags: []string? @with @default(null)\n\
    \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep) \
     @header(\"K\", .tags)\n\
     }\n" ^ wire
  in
  Alcotest.(check bool)
    "nullable list header value" true
    (List.mem "TC0021" (codes src))

(* @body's ctor mapper rejects a bare non-ref/non-string/non-ctor value
   (here an int) as a field value. *)
let body_ctor_field_literal_int_rejected () =
  expect "int field value in a @body ctor" [ "TC0044" ]
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(input: r): r\n\
     \    @http(method: \"POST\", path: \"/\", endpoint: .ep)\n\
     \    @body(r { y: 5 })\n\
      }\n" ^ wire)

(* @body's ctor mapper rejects nesting a second ctor inside a field. *)
let body_ctor_field_nested_ctor_rejected () =
  expect "nested ctor in a @body ctor field" [ "TC0044" ]
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(input: r): r\n\
     \    @http(method: \"POST\", path: \"/\", endpoint: .ep)\n\
     \    @body(r { y: r { y: \"x\" } })\n\
      }\n" ^ wire)

(* A plain string literal is a legal @body ctor field value. *)
let body_ctor_field_literal_string_accepted () =
  expect "string field value in a @body ctor" []
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(input: r): r\n\
     \    @http(method: \"POST\", path: \"/\", endpoint: .ep)\n\
     \    @body(r { y: \"literal\" })\n\
      }\n" ^ wire)

(* An entry op's own parameter, referenced directly as the endpoint, satisfies
   the same typed-ref check an entry field does (the [Entry_scope.RParam]
   branch, distinct from the [RField] one every other endpoint test here
   exercises). *)
let entry_op_param_as_endpoint_accepted () =
  expect "op's own string param as endpoint" []
    ("pub struct c {\n\
     \  op o(ep: string): r @http(method: \"GET\", path: \"/\", endpoint: .ep)\n\
      }\n" ^ wire)

(* Same, for a member of the op's own struct-typed parameter (the
   [Entry_scope.RParam (Member _)] branch, distinct from the whole-param
   branch above). *)
let entry_op_param_member_as_endpoint_accepted () =
  expect "op's own param member as endpoint" []
    ("struct cfg_type { host: string }\n\
      pub struct c {\n\
     \  op o(cfg: cfg_type): r @http(method: \"GET\", path: \"/\", endpoint: \
      .cfg.host)\n\
      }\n" ^ wire)

(* An entry op's endpoint: as a literal/template string (not a field
   reference) is the [value_position_diags] path, clean when it carries no
   input placeholder. *)
let entry_op_endpoint_literal_string_accepted () =
  expect "literal string endpoint" []
    ("pub struct c {\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: \
      \"https://fixed.example.com\")\n\
      }\n" ^ wire)

(* endpoint: rejects a bare literal of any other kind (here, an int). *)
let entry_op_endpoint_wrong_literal_kind_rejected () =
  expect "int endpoint literal" [ "TC0043" ]
    ("pub struct c {\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: 5)\n\
      }\n" ^ wire)

(* An endpoint: ref that does not resolve at all is already reported once
   (as an unknown field reference); the endpoint-type check itself stays
   silent rather than piling on a second diagnostic for the same ref. *)
let entry_op_endpoint_unknown_ref_reported_once () =
  expect "unknown endpoint ref" [ "TC0038" ]
    ("pub struct c {\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .bogus)\n\
      }\n" ^ wire)

(* Same shape for @timeout: an unresolvable ref draws only the one
   unknown-field diagnostic, not a second one from the typed-ref check. *)
let entry_op_timeout_unknown_ref_reported_once () =
  expect "unknown timeout ref" [ "TC0038" ]
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep) \
      @timeout(.bogus)\n\
      }\n" ^ wire)

(* An @http trait with no path: at all (e.g. bound purely by method:) draws
   no path diagnostics -- there is nothing to check. *)
let entry_op_http_without_path_draws_no_path_diagnostics () =
  expect "no path key" []
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", endpoint: .ep)\n\
      }\n" ^ wire)

(* An entry op's endpoint: as a template carrying the legacy @http-path-only
   input placeholder ("{name}") is rejected outside the path position. *)
let entry_op_endpoint_input_placeholder_rejected () =
  expect "input placeholder in endpoint" [ "TC0044" ]
    ("pub struct c {\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: \"{oops}\")\n\
      }\n" ^ wire)

(* An @http path: given as a field reference (rather than a literal/template)
   is the [value_position_diags] ARef branch, clean on its own. *)
let entry_op_path_as_ref_accepted () =
  expect "path as a field reference" []
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: .ep, endpoint: .ep)\n\
      }\n" ^ wire)

(* path: rejects a bare literal of any other kind (here, an int). *)
let entry_op_path_wrong_literal_kind_rejected () =
  expect "int path literal" [ "TC0044" ]
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: 5, endpoint: .ep)\n\
      }\n" ^ wire)

(* A loose op's own parameter, referenced by @header/@query, resolves the
   same way [check_loose_op]'s local [resolve] wraps [resolve_param]'s
   Whole/Member variants for the shared @header/@query shape checks. *)
let loose_op_param_in_header_accepted () =
  expect "loose op param in a header value" []
    "struct w_ty { x: string }\n\
     op o(w: w_ty): w_ty @http(method: \"GET\", path: \"/\") @header(\"K\", \
     .w.x)"

(* A loose op's endpoint: with a non-ref value (here a literal) still hits
   the "belongs to an entry" rejection, distinct from the field-reference
   case every other loose-op endpoint test here exercises. *)
let loose_op_endpoint_literal_rejected () =
  expect "loose op literal endpoint" [ "TC0044" ]
    "struct w { x: string }\n\
     op o(w): w @http(method: \"GET\", path: \"/\", endpoint: \"fixed\")"

(* @timeout referencing the loose op's own parameter is legal (the one
   carve-out a loose op has, mirroring [entry_op_param_as_endpoint_accepted]
   but for the loose-op path). *)
let loose_op_param_via_timeout_accepted () =
  expect "loose op param via timeout" []
    "struct w_ty { t: duration }\n\
     op o(w: w_ty): w_ty @http(method: \"GET\", path: \"/\") @timeout(.w.t)"

(* A loose op's @http path: referencing its own parameter goes through
   [check_path_presence] the same way an entry op's does. *)
let loose_op_path_presence_checked () =
  expect "loose op path presence" []
    "struct w_ty { id: string }\n\
     op o(w: w_ty): w_ty @http(method: \"GET\", path: \"/x/{.w.id}\")"

(* A loose op's @http with no path: key at all draws no path diagnostics,
   same as the entry-op case above. *)
let loose_op_http_without_path_draws_nothing () =
  expect "loose op no path key" []
    "struct w_ty { id: string }\nop o(w: w_ty): w_ty @http(method: \"GET\")"

(* A loose op's @header referencing an unresolvable ref (neither its own
   param nor an entry field, since loose ops have none) draws the one
   already-reported diagnostic; the shared kv-shape resolver itself stays
   silent on the unresolved ref. *)
let loose_op_header_unresolvable_ref_reported_once () =
  expect "loose op header unresolvable ref" [ "TC0044" ]
    "struct w_ty { x: string }\n\
     op o(w: w_ty): w_ty @http(method: \"GET\", path: \"/\") @header(\"K\", \
     .bogus)"

(* A loose op's @header referencing the whole parameter (not one of its
   members) resolves through the [Whole] branch, distinct from the [Member]
   branch [loose_op_param_in_header_accepted] exercises. *)
let loose_op_whole_param_in_header_accepted () =
  expect "loose op whole param in a header value" []
    "struct w_ty { x: string }\n\
     op o(w: w_ty): w_ty @http(method: \"GET\", path: \"/\") @header(\"K\", .w)"

(* @timeout referencing a ref that resolves to neither the loose op's own
   param nor (loose ops have no fields) anything else draws only the one
   already-reported unknown-ref diagnostic. *)
let loose_op_timeout_unresolvable_ref_reported_once () =
  expect "loose op timeout unresolvable ref" [ "TC0044" ]
    "struct w_ty { t: duration }\n\
     op o(w: w_ty): w_ty @http(method: \"GET\", path: \"/\") @timeout(.bogus)"

(* An entry op's @http path: referencing its own parameter (as opposed to an
   entry field) goes through [resolve_ty]'s [RParam (Whole _)] branch. *)
let entry_op_path_via_own_param_accepted () =
  expect "entry op path via own param" []
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(p: string): r @http(method: \"GET\", path: \"/x/{.p}\", endpoint: \
      .ep)\n\
      }\n" ^ wire)

(* A @header/@query key given as a field reference (rather than a string
   literal/template) is a legal key shape on its own. *)
let entry_op_header_key_as_ref_accepted () =
  expect "header key as a field reference" []
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep) \
      @header(.ep, .ep)\n\
      }\n" ^ wire)

(* Unlike endpoint:, an entry op's @timeout/@retry may only ever name an
   entry field (the [RParam _] branch always errors here, regardless of the
   param's own type) -- the op's own parameter has no such carve-out the way
   it does on a loose op. *)
let entry_op_own_param_via_timeout_rejected () =
  expect "op's own param via timeout is always rejected" [ "TC0044" ]
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(t: duration): r @http(method: \"GET\", path: \"/\", endpoint: \
      .ep) @timeout(.t)\n\
      }\n" ^ wire)

let unterminated_path_placeholder_diagnosed () =
  let diags =
    check
      ("pub struct c {\n\
       \  ep: string @env(\"EP\")\n\
       \  op o(): r @http(method: \"GET\", path: \"/x{oops\", endpoint: .ep)\n\
        }\n" ^ wire)
  in
  Alcotest.(check bool)
    "unterminated brace reported" true
    (List.exists
       (fun (d : Diagnostic.t) -> contains d.message "unterminated '{'")
       diags)

let duplicate_trait_is_a_warning () =
  let diags = check "@doc(\"a\")\n@doc(\"b\")\nstruct s { x: string }" in
  match diags with
  | [ d ] ->
      Alcotest.(check bool)
        "warning severity" true
        (d.severity = Diagnostic.Warning)
  | _ -> Alcotest.fail "expected exactly one diagnostic"

let source_marker_args_diagnosed () =
  let lower_msgs src =
    let file, _ = Parser.parse src in
    let diags = ref [] in
    ignore (Lower.lower_file ~module_name:"m" ~diags file);
    List.map (fun (d : Diagnostic.t) -> d.message) !diags
  in
  let has src frag =
    Alcotest.(check bool)
      frag true
      (List.exists (fun m -> contains m frag) (lower_msgs src))
  in
  has "struct s { a: string @arg(5), op ping() }" "@arg takes no arguments";
  has "struct s { a: string @with(x), op ping() }" "@with takes no arguments";
  has "struct s { a: string @default(\"x\", \"y\"), op ping() }"
    "@default takes a single value"

(* An explicit JSON "pattern": null is the mandatory "null" arm of an
   optional subject, distinct from an absent "pattern" key (the wildcard). *)
let null_pattern_decodes_as_null_arm () =
  let null_marker = `Assoc [ ("null", `Bool true) ] in
  match
    Ir_json.decode_select
      (`Assoc
         [
           ("subject", `List [ `String "v" ]);
           ( "arms",
             `List
               [
                 `Assoc
                   [
                     ("pattern", null_marker);
                     ("value", `Assoc [ ("lit", `Null) ]);
                   ];
               ] );
         ])
  with
  | Ok { arms = [ { arm_pattern = Some p; _ } ]; _ } when p = null_marker -> ()
  | Ok _ -> Alcotest.fail "null pattern did not decode as the null arm"
  | Error e -> Alcotest.failf "decode failed: %s" e

let absent_pattern_decodes_as_wildcard () =
  match
    Ir_json.decode_select
      (`Assoc
         [
           ("subject", `List [ `String "v" ]);
           ("arms", `List [ `Assoc [ ("value", `Assoc [ ("lit", `Null) ]) ] ]);
         ])
  with
  | Ok { arms = [ { arm_pattern = None; _ } ]; _ } -> ()
  | Ok _ -> Alcotest.fail "an absent pattern must decode as the wildcard"
  | Error e -> Alcotest.failf "decode failed: %s" e

(* ── Nested boundaries, typed arms, loose header shapes ────────────────── *)

let nested_config_in_wire_list_rejected () =
  expect "list of config in a wire struct" [ "TC0034" ]
    "struct conf { k: string @env(\"K\") }\nstruct w { c: []conf }"

let nested_config_in_generic_entry_field_rejected () =
  (* The field also declares no source (a generic application is not the
     composition point), so both errors are honest. *)
  expect "config inside a generic application" [ "TC0037"; "TC0034" ]
    ("struct conf { k: string @env(\"K\") }\nstruct page[t] { items: []t }\n"
   ^ entry "  p: page[conf]")

let nested_config_in_wire_map_rejected () =
  expect "map of config in a wire struct" [ "TC0034" ]
    "struct conf { k: string @env(\"K\") }\nstruct w { m: map[string]conf }"

let arm_literal_wrong_field_type_rejected () =
  expect "string arm into an i32 field" [ "TC0040" ]
    (entry "  v: string @env(\"V\")\n  x: i32 = match .v { _ => \"s\" }")

let arm_ref_wrong_field_type_rejected () =
  expect "i32 ref arm into a string field" [ "TC0040" ]
    (entry
       "  v: string @env(\"V\")\n\
       \  n: i32 @with @default(1)\n\
       \  x: string = match .v { _ => .n }")

let loose_header_literal_int_rejected () =
  expect "int header value on a loose op" [ "TC0044" ]
    "struct w { x: string }\n\
     op o(w): w @http(method: \"GET\", path: \"/\") @header(\"K\", 5)"

let loose_header_input_placeholder_key_rejected () =
  expect "input placeholder key on a loose op" [ "TC0044" ]
    "struct w { x: string }\n\
     op o(w): w @http(method: \"GET\", path: \"/\") @header(\"X-{id}\", \"v\")"

let nullable_composed_config_classifies () =
  (* The bind flip reads through the nullable spelling, so the only error is
     the nullable field itself, not a misleading misplaced-@bind one. *)
  expect "nullable composition point" [ "TC0046" ]
    ("struct conf { api_key: string }\n"
    ^ entry "  settings: conf? @bind(api_key, .ep)")

let degenerate_placeholders_diagnosed () =
  let lower_msgs src =
    let file, _ = Parser.parse src in
    let diags = ref [] in
    ignore (Lower.lower_file ~module_name:"m" ~diags file);
    List.map (fun (d : Diagnostic.t) -> d.message) !diags
  in
  let has src frag =
    Alcotest.(check bool)
      frag true
      (List.exists (fun m -> contains m frag) (lower_msgs src))
  in
  has "struct s { a: string @format(\"x{}\"), op ping() }"
    "empty '{}' placeholder";
  has "struct s { a: string @format(\"x{.}\"), op ping() }"
    "empty field reference"

let arm_int_into_string_rejected () =
  expect "int arm into a string field" [ "TC0040" ]
    (entry "  v: string @env(\"V\")\n  x: string = match .v { _ => 5 }")

let arm_bool_into_string_rejected () =
  expect "bool arm into a string field" [ "TC0040" ]
    (entry "  v: string @env(\"V\")\n  x: string = match .v { _ => true }")

let arm_bare_name_into_string_rejected () =
  expect "bare name arm into a string field" [ "TC0040" ]
    (entry "  v: string @env(\"V\")\n  x: string = match .v { _ => banana }")

let arm_wrong_enum_case_rejected () =
  expect "wrong case arm into an enum field" [ "TC0040" ]
    ("enum lvl { low, high }\n"
    ^ entry "  v: string @env(\"V\")\n  x: lvl = match .v { _ => nope }")

let arm_matching_enum_case_accepted () =
  expect "matching case arm into an enum field" []
    ("enum lvl { low, high }\n"
    ^ entry "  v: string @env(\"V\")\n  x: lvl = match .v { _ => low }")

let () =
  Alcotest.run "entries_position"
    [
      ( "round3",
        [
          Alcotest.test_case "arm int into string" `Quick
            arm_int_into_string_rejected;
          Alcotest.test_case "arm bool into string" `Quick
            arm_bool_into_string_rejected;
          Alcotest.test_case "arm bare name into string" `Quick
            arm_bare_name_into_string_rejected;
          Alcotest.test_case "arm wrong enum case" `Quick
            arm_wrong_enum_case_rejected;
          Alcotest.test_case "arm matching enum case" `Quick
            arm_matching_enum_case_accepted;
          Alcotest.test_case "config in wire list" `Quick
            nested_config_in_wire_list_rejected;
          Alcotest.test_case "config in generic arg" `Quick
            nested_config_in_generic_entry_field_rejected;
          Alcotest.test_case "config in wire map" `Quick
            nested_config_in_wire_map_rejected;
          Alcotest.test_case "arm literal type" `Quick
            arm_literal_wrong_field_type_rejected;
          Alcotest.test_case "arm ref type" `Quick
            arm_ref_wrong_field_type_rejected;
          Alcotest.test_case "loose header int value" `Quick
            loose_header_literal_int_rejected;
          Alcotest.test_case "loose header input key" `Quick
            loose_header_input_placeholder_key_rejected;
          Alcotest.test_case "nullable composed config" `Quick
            nullable_composed_config_classifies;
          Alcotest.test_case "degenerate placeholders" `Quick
            degenerate_placeholders_diagnosed;
        ] );
      ( "typed-refs",
        [
          Alcotest.test_case "bind type mismatch" `Quick
            bind_type_mismatch_rejected;
          Alcotest.test_case "env ref must be string" `Quick
            env_ref_must_be_string;
          Alcotest.test_case "duplicate entry op" `Quick
            duplicate_entry_op_rejected;
          Alcotest.test_case "header value template" `Quick
            header_value_template_accepted;
          Alcotest.test_case "header key input placeholder" `Quick
            header_key_input_placeholder_rejected;
          Alcotest.test_case "header key crlf" `Quick header_key_crlf_rejected;
          Alcotest.test_case "header nullable list value" `Quick
            header_nullable_list_value_rejected;
          Alcotest.test_case "body ctor int literal" `Quick
            body_ctor_field_literal_int_rejected;
          Alcotest.test_case "body ctor nested ctor" `Quick
            body_ctor_field_nested_ctor_rejected;
          Alcotest.test_case "body ctor string literal" `Quick
            body_ctor_field_literal_string_accepted;
          Alcotest.test_case "op param as endpoint" `Quick
            entry_op_param_as_endpoint_accepted;
          Alcotest.test_case "op param member as endpoint" `Quick
            entry_op_param_member_as_endpoint_accepted;
          Alcotest.test_case "endpoint literal string" `Quick
            entry_op_endpoint_literal_string_accepted;
          Alcotest.test_case "endpoint wrong literal kind" `Quick
            entry_op_endpoint_wrong_literal_kind_rejected;
          Alcotest.test_case "endpoint unknown ref reported once" `Quick
            entry_op_endpoint_unknown_ref_reported_once;
          Alcotest.test_case "timeout unknown ref reported once" `Quick
            entry_op_timeout_unknown_ref_reported_once;
          Alcotest.test_case "http without path draws nothing" `Quick
            entry_op_http_without_path_draws_no_path_diagnostics;
          Alcotest.test_case "endpoint input placeholder" `Quick
            entry_op_endpoint_input_placeholder_rejected;
          Alcotest.test_case "path as a field reference" `Quick
            entry_op_path_as_ref_accepted;
          Alcotest.test_case "path wrong literal kind" `Quick
            entry_op_path_wrong_literal_kind_rejected;
          Alcotest.test_case "loose op param in header" `Quick
            loose_op_param_in_header_accepted;
          Alcotest.test_case "loose op literal endpoint" `Quick
            loose_op_endpoint_literal_rejected;
          Alcotest.test_case "loose op param via timeout" `Quick
            loose_op_param_via_timeout_accepted;
          Alcotest.test_case "loose op path presence checked" `Quick
            loose_op_path_presence_checked;
          Alcotest.test_case "loose op http without path draws nothing" `Quick
            loose_op_http_without_path_draws_nothing;
          Alcotest.test_case "loose op header unresolvable ref" `Quick
            loose_op_header_unresolvable_ref_reported_once;
          Alcotest.test_case "loose op whole param in header" `Quick
            loose_op_whole_param_in_header_accepted;
          Alcotest.test_case "loose op timeout unresolvable ref" `Quick
            loose_op_timeout_unresolvable_ref_reported_once;
          Alcotest.test_case "entry op path via own param" `Quick
            entry_op_path_via_own_param_accepted;
          Alcotest.test_case "header key as a field reference" `Quick
            entry_op_header_key_as_ref_accepted;
          Alcotest.test_case "op param via timeout always rejected" `Quick
            entry_op_own_param_via_timeout_rejected;
          Alcotest.test_case "unterminated path placeholder" `Quick
            unterminated_path_placeholder_diagnosed;
          Alcotest.test_case "duplicate trait is a warning" `Quick
            duplicate_trait_is_a_warning;
          Alcotest.test_case "source marker args" `Quick
            source_marker_args_diagnosed;
          Alcotest.test_case "null pattern decodes as null arm" `Quick
            null_pattern_decodes_as_null_arm;
          Alcotest.test_case "absent pattern decodes as wildcard" `Quick
            absent_pattern_decodes_as_wildcard;
        ] );
      ( "silent-drop",
        [
          Alcotest.test_case "wire member match" `Quick
            wire_member_match_rejected;
          Alcotest.test_case "wire member format" `Quick
            wire_member_format_rejected;
          Alcotest.test_case "wire member transform" `Quick
            wire_member_transform_rejected;
          Alcotest.test_case "wire member bind" `Quick wire_member_bind_rejected;
          Alcotest.test_case "loose literal timeout" `Quick
            loose_op_literal_timeout_rejected;
          Alcotest.test_case "loose literal retry" `Quick
            loose_op_literal_retry_rejected;
          Alcotest.test_case "env with placeholder" `Quick
            env_with_placeholder_rejected;
          Alcotest.test_case "int header value" `Quick
            header_literal_int_value_rejected;
          Alcotest.test_case "absorbed doc duplicate" `Quick
            absorbed_doc_duplicate_rejected;
          Alcotest.test_case "duplicate decl doc" `Quick
            duplicate_doc_on_decl_rejected;
          Alcotest.test_case "repeatable traits legal" `Quick
            repeatable_traits_stay_legal;
          Alcotest.test_case "config boundary hint" `Quick
            config_boundary_names_composition_point;
        ] );
    ]
