open Tono_frontend

(* Typecheck coverage for the "ext"/"extern" FFI library block:
   per-extern call/yields/returns/errors closure ([Check_ext_lib.check_decls]),
   the foreign-role wire/surface boundary ([Roles.Foreign] +
   [Check_entries.boundary_ty]), and the cross-file closed accounting
   (decision K, [Check_ext_lib_project.check_project]). Mirrors [typecheck_test.ml]'s
   [check]/[codes] helpers. *)

(* Substring test for message assertions, mirroring [module_test.ml]'s helper
   of the same name. *)
let contains ~sub s =
  let n = String.length sub and m = String.length s in
  let rec go i = i + n <= m && (String.sub s i n = sub || go (i + 1)) in
  n = 0 || go 0

let check src =
  let file, _ = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let _, tc = Typecheck.check_module ~file m in
  tc

(* [check] only inspects post-parse diagnostics; a test relying on a
   specific shape surviving the parse (the entry-field carve-out, in
   particular) must assert this first, or a parse-recovery cascade that
   silently drops the field under test would make the assertion pass for
   the wrong reason. *)
let parse_diag_count src =
  let _, pdiags = Parser.parse src in
  List.length pdiags

let codes src = List.filter_map (fun (d : Diagnostic.t) -> d.code) (check src)
let has code src = List.mem code (codes src)

(* [Typecheck.check_module] alone never runs the cross-file closed
   accounting ([Check_ext_lib_project.check_project]); it needs the whole-project
   entry points, same as [Modules.build]. *)
let compile_codes src =
  let _, ds = Tono_frontend.compile src in
  List.filter_map (fun (d : Diagnostic.t) -> d.Diagnostic.code) ds

let project_codes files =
  let _, ds = Tono_frontend.compile_project files in
  List.filter_map (fun (d : Diagnostic.t) -> d.Diagnostic.code) ds

let line_of code src =
  let d =
    List.find (fun (d : Diagnostic.t) -> d.code = Some code) (check src)
  in
  d.span.start.line

(* ── Member grammar: a trait may lead or trail the "=" value ────────────── *)

(* The ext/extern injectable-handle idiom spells the trait before the value
   ("field: type @with = ns.fn(args)"); every other member value form in the
   language spells it after ("field: type = match ... @trait"). Both must
   parse to the same [mtraits], since nothing else in the checker
   distinguishes where a trait was written. *)
let member_traits_of src =
  let file, pdiags = Parser.parse src in
  Alcotest.(check int) "parses clean" 0 (List.length pdiags);
  match file.Ast.decls with
  | [ { Ast.dkind = Ast.DStruct { members = [ m ]; _ }; _ } ] ->
      List.map (fun (t : Ast.trait) -> t.Ast.tname) m.Ast.mtraits
  | _ -> Alcotest.fail "expected exactly one single-member struct"

let leading_and_trailing_trait_order_equivalent () =
  let leading = "struct s { x: string @with = tono_extern_stub.f(.x) }" in
  let trailing = "struct s { x: string = tono_extern_stub.f(.x) @with }" in
  (* Neither example resolves "tono_extern_stub" (there is no such ext
     block); only the member grammar is under test, so we read [mtraits]
     straight off the parsed AST rather than through the typechecker. *)
  Alcotest.(check (list string)) "leading" [ "with" ] (member_traits_of leading);
  Alcotest.(check (list string))
    "trailing" [ "with" ]
    (member_traits_of trailing)

(* ── A clean ext block: baseline for every mutation below ───────────────── *)

let base =
  {|ext lib {
  go { #(github.com/x/y) }

  struct go_cfg { Host: string }

  op load(service: string): app_config {
    go {
      call: #(Load)(service)
      yields: (cfg: go_cfg)
      returns: app_config { endpoint: .cfg.Host }
    }
  }
}

struct app_config { endpoint: string }
|}

let clean () = Alcotest.(check (list string)) "no codes" [] (codes base)

(* ── The chained method's arguments are the call's own ──────────────────── *)

(* A parameter consumed only by the method chained on the returned object
   is consumed (no TC0078), and an undeclared name there is TC0070 like
   anywhere else in the line: the chain is part of call:, not a second
   binding with rules of its own. *)
let chain_args_are_call_args () =
  let consumed =
    {|ext lib {
  go { #(github.com/x/y) }

  op load(key: string, mode: string): string {
    go { call: #(Get)(key).#(Result)(mode) }
  }
}
|}
  in
  Alcotest.(check int) "parses clean" 0 (parse_diag_count consumed);
  Alcotest.(check (list string)) "no codes" [] (codes consumed);
  let unknown =
    {|ext lib {
  go { #(github.com/x/y) }

  op load(key: string): string {
    go { call: #(Get)(key).#(Result)(bogus) }
  }
}
|}
  in
  Alcotest.(check bool) "unknown param in the chain" true (has "TC0070" unknown)

(* ── TC0070: call: names an undeclared logical parameter ────────────────── *)

let unknown_call_param () =
  let src =
    {|ext lib {
  go { #(github.com/x/y) }

  struct go_cfg { Host: string }

  op load(service: string): app_config {
    go {
      call: #(Load)(bogus)
      yields: (cfg: go_cfg)
      returns: app_config { endpoint: .cfg.Host }
    }
  }
}

struct app_config { endpoint: string }
|}
  in
  Alcotest.(check bool) "unknown param" true (has "TC0070" src)

(* ── TC0071: a ctor projection disagrees with the declared foreign struct ─ *)

let ctor_field_type_mismatch () =
  let src =
    {|ext lib {
  go { #(github.com/x/y) }

  struct go_cfg { Host: string }
  struct go_opts { Num: string }

  op load(count: i64): app_config {
    go {
      call: #(Load)(go_opts { Num: count })
      yields: (cfg: go_cfg)
      returns: app_config { endpoint: .cfg.Host }
    }
  }
}

struct app_config { endpoint: string }
|}
  in
  Alcotest.(check bool) "ctor field type mismatch" true (has "TC0071" src)

let ctor_unknown_field () =
  let src =
    {|ext lib {
  go { #(github.com/x/y) }

  struct go_cfg { Host: string }
  struct go_opts { Num: i64 }

  op load(count: i64): app_config {
    go {
      call: #(Load)(go_opts { Bogus: count })
      yields: (cfg: go_cfg)
      returns: app_config { endpoint: .cfg.Host }
    }
  }
}

struct app_config { endpoint: string }
|}
  in
  Alcotest.(check bool) "ctor unknown field" true (has "TC0071" src)

(* ── TC0072: a yields: position nothing in returns:/errors: consumes ────── *)

let dead_yields_position () =
  let src =
    {|ext lib {
  go { #(github.com/x/y) }

  struct go_cfg { Host: string, Extra: string }

  op load(service: string): app_config {
    go {
      call: #(Load)(service)
      yields: (cfg: go_cfg, extra: go_cfg)
      returns: app_config { endpoint: .cfg.Host }
    }
  }
}

struct app_config { endpoint: string }
|}
  in
  Alcotest.(check bool) "dead position" true (has "TC0072" src);
  (* The diagnostic points at the unconsumed position's own span
     ("extra: go_cfg"), not at the whole yields: list or the extern. *)
  Alcotest.(check int) "points at 'extra'" 9 (line_of "TC0072" src)

(* The reserved error position is consumed by the boundary itself (the
   op's declared errors and the contract wrap read it), so it never counts
   as dead. *)
let error_position_is_consumed () =
  let src =
    {|ext lib {
  go { #(github.com/x/y) }

  struct go_cfg { Host: string }

  op load(service: string): app_config {
    go {
      call: #(Load)(service)
      yields: (cfg: go_cfg, err: error)
      returns: app_config { endpoint: .cfg.Host }
    }
  }
}

struct app_config { endpoint: string }
|}
  in
  Alcotest.(check bool) "error position is consumed" false (has "TC0072" src)

(* ── TC0073: more than one "error"-typed yields: position ───────────────── *)

let two_error_positions () =
  let src =
    {|ext lib {
  go { #(github.com/x/y) }

  struct go_ack { OK: bool }

  @errors(overloaded)
  op send(topic: string): ack {
    go {
      call: #(Send)(topic)
      yields: (a: error, b: error)
      returns: ack { accepted: true }
    }
  }
}

pub struct ack { accepted: bool }
@status(503)
pub struct overloaded { message: string }
|}
  in
  Alcotest.(check bool) "two error positions" true (has "TC0073" src)

(* ── TC0074: returns: with no yields: to project from ───────────────────── *)

let returns_without_yields () =
  let src =
    {|ext lib {
  go { #(github.com/x/y) }

  op load(service: string): app_config {
    go {
      call: #(Load)(service)
      returns: app_config { endpoint: "x" }
    }
  }
}

struct app_config { endpoint: string }
|}
  in
  Alcotest.(check bool) "returns without yields" true (has "TC0074" src)

(* ── TC0075: returns: builds a type other than the extern's own return ──── *)

let returns_wrong_type () =
  let src =
    {|ext lib {
  go { #(github.com/x/y) }

  struct go_cfg { Host: string }

  op load(service: string): app_config {
    go {
      call: #(Load)(service)
      yields: (cfg: go_cfg)
      returns: other_config { endpoint: .cfg.Host }
    }
  }
}

struct app_config { endpoint: string }
struct other_config { endpoint: string }
|}
  in
  Alcotest.(check bool) "returns wrong type" true (has "TC0075" src)

(* ── TC0076: a returns: field ref's head isn't a declared yields: name ──── *)

let returns_ref_unknown () =
  let src =
    {|ext lib {
  go { #(github.com/x/y) }

  struct go_cfg { Host: string }

  op load(service: string): app_config {
    go {
      call: #(Load)(service)
      yields: (cfg: go_cfg)
      returns: app_config { endpoint: .bogus.Host }
    }
  }
}

struct app_config { endpoint: string }
|}
  in
  Alcotest.(check bool) "unknown returns ref" true (has "TC0076" src)

(* ── TC0077: errors: sentinel maps to a type that does not resolve ──────── *)

let error_sentinel_unknown () =
  let src =
    {|ext lib {
  go { #(github.com/x/y) }

  struct go_ack { OK: bool }

  @errors(nope)
  op send(topic: string): ack {
    go {
      call: #(Send)(topic)
      yields: (a: go_ack, err: error)
      returns: ack { accepted: .a.OK }
    }
  }
}

pub struct ack { accepted: bool }
|}
  in
  Alcotest.(check bool) "unknown error sentinel" true (has "TC0077" src)

(* ── TC0078: a logical parameter this language's call: never consumes ───── *)

let param_unconsumed () =
  let src =
    {|ext lib {
  go { #(github.com/x/y) }

  struct go_cfg { Host: string }

  op load(service: string, region: string): app_config {
    go {
      call: #(Load)(service)
      yields: (cfg: go_cfg)
      returns: app_config { endpoint: .cfg.Host }
    }
  }
}

struct app_config { endpoint: string }
|}
  in
  Alcotest.(check bool) "param unconsumed" true (has "TC0078" src)

(* TC0088 ('ctx' only applies to a foreign handle's own method call) has its
   own test file, [ctx_marker_test.ml], for the same file-size-cap reason
   noted below for [ext_lib_collisions_test.ml]/[op_impl_test.ml]. *)

(* ── Foreign-role boundary (TC0034): never wire, op input/output, surface ─ *)

let foreign_struct_as_op_output () =
  let src = base ^ "\nop fetch(): go_cfg\n" in
  Alcotest.(check bool) "foreign struct as op output" true (has "TC0034" src);
  (* TC0034 alone: a foreign name is not a Symtab shape, but it is not
     unknown either, so it must not also draw TC0001. *)
  Alcotest.(check bool) "no redundant TC0001" false (has "TC0001" src)

let foreign_struct_as_op_input () =
  let src = base ^ "\nop fetch(x: go_cfg): app_config\n" in
  Alcotest.(check bool) "foreign struct as op input" true (has "TC0034" src)

let foreign_struct_as_wire_member () =
  let src = base ^ "\nstruct holder { c: go_cfg }\n" in
  Alcotest.(check bool) "foreign struct in wire member" true (has "TC0034" src)

(* An opaque handle used qualified as an op input/output is closed the same
   way; only a bare entry field composes it. *)
let opaque_handle_as_op_output () =
  let src =
    {|ext bus {
  go { #(github.com/x/bus) }

  struct go_ack { OK: bool }

  struct publisher {
    op send(topic: string): ack {
      go {
        call: #(Send)(topic)
        yields: (a: go_ack)
        returns: ack { accepted: .a.OK }
      }
    }
  }

  op connect(endpoint: string): publisher {
    go { call: #(Connect)(endpoint) }
  }
}

pub struct ack { accepted: bool }

op fetch(): bus.publisher
|}
  in
  Alcotest.(check bool) "opaque handle as op output" true (has "TC0034" src)

(* The one carve-out: a bare qualified field on an entry composes the
   handle legally (no diagnostic). *)
let opaque_handle_as_entry_field_ok () =
  let src =
    {|ext bus {
  go { #(github.com/x/bus) }

  struct go_ack { OK: bool }

  struct publisher {
    op send(topic: string): ack {
      go {
        call: #(Send)(topic)
        yields: (a: go_ack)
        returns: ack { accepted: .a.OK }
      }
    }
  }

  op connect(endpoint: string): publisher {
    go { call: #(Connect)(endpoint) }
  }
}

pub struct ack { accepted: bool }

pub struct client {
  endpoint: string @arg
  bus: bus.publisher @with = bus.connect(.endpoint)

  op fetch(): ack @http(method: "GET", path: "/x", endpoint: .endpoint)
}
|}
  in
  Alcotest.(check int) "parses clean" 0 (parse_diag_count src);
  Alcotest.(check (list string))
    "no boundary diagnostics" []
    (List.filter (String.equal "TC0034") (codes src))

(* Foreign generic type instantiation: see extern_instance_test.ml. *)

(* A yields: position with no returns: at all (not merely one that omits a
   ref to this position) still has nothing to consume it. *)
let yields_with_no_returns_at_all_is_dead () =
  let src =
    {|ext lib {
  go { #(github.com/x/y) }

  struct go_cfg { Host: string }

  op load(service: string): app_config {
    go {
      call: #(Load)(service)
      yields: (cfg: go_cfg)
    }
  }
}

struct app_config { endpoint: string }
|}
  in
  Alcotest.(check bool)
    "dead position, no returns at all" true (has "TC0072" src)

(* ── Decision K: cross-file closed accounting ────────────────────────────── *)

let split_lib_a =
  {|ext lib {
  go { #(github.com/x/y) }

  struct go_cfg { Host: string }

  op load(service: string): app_config {
    go {
      call: #(Load)(service)
      yields: (cfg: go_cfg)
      returns: app_config { endpoint: .cfg.Host }
    }
  }
}

struct app_config { endpoint: string }
|}

let split_lib_b =
  {|ext lib {
  struct go_creds { Secret: string }

  op token(): string {
    go { call: #(Token)() }
  }
}
|}

let split_valid_two_files () =
  Alcotest.(check (list string))
    "no codes" []
    (project_codes [ ("a", split_lib_a); ("b", split_lib_b) ])

let split_valid_within_one_file () =
  (* Decision K applies just as well within a single file: two "ext lib"
     blocks with the same name are one library, and closing the account is
     the same check regardless of file boundaries. *)
  let src = split_lib_a ^ "\n" ^ split_lib_b in
  Alcotest.(check (list string)) "no codes" [] (compile_codes src)

let conflicting_module_path () =
  let b =
    {|ext lib {
  go { #(github.com/x/other) }

  op token(): string {
    go { call: #(Token)() }
  }
}
|}
  in
  let _, ds = Tono_frontend.compile_project [ ("a", split_lib_a); ("b", b) ] in
  let messages =
    List.filter_map
      (fun (d : Diagnostic.t) ->
        if d.Diagnostic.code = Some "TC0079" then Some d.Diagnostic.message
        else None)
      ds
  in
  Alcotest.(check bool) "conflicting module path" true (messages <> []);
  (* Each diagnostic must name both paths and both files: the reader should
     never have to diff two separate messages to see what conflicts with
     what. *)
  List.iter
    (fun msg ->
      Alcotest.(check bool)
        (msg ^ ": names both paths")
        true
        (contains ~sub:"github.com/x/y" msg
        && contains ~sub:"github.com/x/other" msg);
      Alcotest.(check bool)
        (msg ^ ": names both files")
        true
        (contains ~sub:"'a'" msg && contains ~sub:"'b'" msg))
    messages

let duplicate_extern_name_across_files () =
  let b =
    {|ext lib {
  op load(x: string): string {
    go { call: #(Other)(x) }
  }
}
|}
  in
  Alcotest.(check bool)
    "duplicate extern name" true
    (List.mem "TC0080" (project_codes [ ("a", split_lib_a); ("b", b) ]))

let lang_block_without_module () =
  let b =
    {|ext lib {
  op token(): string {
    ts { call: #(token)() }
  }
}
|}
  in
  Alcotest.(check bool)
    "lang block without module" true
    (List.mem "TC0081" (project_codes [ ("a", split_lib_a); ("b", b) ]))

(* Module-local foreign-name collisions (TC0080's in-file half), the
   opaque-handle-is-not-generic check (TC0005), and nested call: arg shape
   coverage have their own test file, [ext_lib_collisions_test.ml], split out
   to stay under the file-size cap.

   The op's own "impl .field.method(args)" body has its own test
   file, [op_impl_test.ml], for the same reason. *)

(* ── The ext/extern reference example compiles clean ────────────────────── *)

(* A verbatim transcription of the ext/extern design's reference example,
   with one adjustment unrelated to this checker: "overloaded" gains @status
   (TC0015, a pre-existing rule independent of ext/extern; the original
   example omits it). Both syntax gaps found while building this feature are closed:
   the member grammar accepts a trait on either side of the "=" value (the
   "bus" field's own "@with = ns.fn(...)"), and "publish"'s own
   "impl .bus.send(...)" op body parses and typechecks against "publisher"'s
   declared "send" method. *)
let reference_example =
  {|import tono.http

ext companyconfig {
  go { #(github.com/company/config) }
  ts { #(@company/config) }

  struct go_config { Host: string, DevHost: string, Env: string, Credentials: go_creds }
  struct go_creds  { Secret: string }
  struct ts_config { host: string, token: string }
  struct ts_opts   { region: string, service: string }

  op load(service: string, region: string): app_config {
    go {
      call: #(Load)(service, region)
      yields: (cfg: go_config)
      returns: app_config {
        endpoint: match .cfg.Env { "prod" => .cfg.Host, _ => .cfg.DevHost }
        token: .cfg.Credentials.Secret
      }
    }
    ts {
      call: #(load)(ts_opts { region: region, service: service })
      yields: (cfg: ts_config)
      returns: app_config { endpoint: .cfg.host, token: .cfg.token }
    }
  }
}

ext companybus {
  go { #(github.com/company/bus) }
  ts { #(@company/bus) }

  struct go_ack { ID: string, OK: bool }
  struct ts_ack { id: string, accepted: bool }

  struct publisher {
    @errors(overloaded)
    op send(topic: string, body: string): ack {
      go {
        call: #(Send)(topic, body)
        yields: (a: go_ack)
        returns: ack { id: .a.ID, accepted: .a.OK }
      }
      ts {
        call: #(send)(topic, body)
        yields: (a: ts_ack)
        returns: ack { id: .a.id, accepted: .a.accepted }
      }
    }
  }

  op connect(endpoint: string, token: string): publisher {
    go { call: #(Connect)(endpoint, token) }
    ts { call: #(connect)(endpoint, token) }
  }
}

struct app_config { endpoint: string, token: string }
struct note_ref   { id: string }

pub struct note { id: string, body: string }
pub struct ack  { id: string, accepted: bool }

@doc("The bus is overloaded.")
@retryable
@status(503)
pub struct overloaded { message: string }

@doc("No note with that id.")
@status(404)
pub struct not_found { message: string }

@doc("Notes SDK: config comes from the internal lib, publishing goes through the bus.")
pub struct client {
  service: string @env("SERVICE_NAME") @default("notes")
  region: string @arg

  config: app_config = companyconfig.load(.service, .region)
  auth: string @format("Bearer {.config.token}")

  bus: companybus.publisher @with = companybus.connect(.config.endpoint, .config.token)

  @http(method: "GET", path: "/notes/{.ref.id}", endpoint: .config.endpoint)
  @header("Authorization", .auth)
  @errors(not_found)
  op fetch(ref: note_ref): note

  @errors(overloaded)
  op publish(payload: note): ack
    impl .bus.send("notes", .payload.body)
}
|}

let reference_example_clean () =
  Alcotest.(check int) "parses clean" 0 (parse_diag_count reference_example);
  let codes = compile_codes reference_example in
  Alcotest.(check (list string)) "no diagnostics" [] codes

let () =
  Alcotest.run "extern_typecheck"
    [
      ("baseline", [ Alcotest.test_case "clean ext block" `Quick clean ]);
      ( "call chain",
        [
          Alcotest.test_case "chain arguments are call arguments" `Quick
            chain_args_are_call_args;
        ] );
      ( "grammar",
        [
          Alcotest.test_case "leading and trailing trait order" `Quick
            leading_and_trailing_trait_order_equivalent;
        ] );
      ( "call",
        [
          Alcotest.test_case "unknown call param" `Quick unknown_call_param;
          Alcotest.test_case "ctor field type mismatch" `Quick
            ctor_field_type_mismatch;
          Alcotest.test_case "ctor unknown field" `Quick ctor_unknown_field;
          Alcotest.test_case "param unconsumed by binding" `Quick
            param_unconsumed;
        ] );
      ( "yields-returns-errors",
        [
          Alcotest.test_case "dead yields position" `Quick dead_yields_position;
          Alcotest.test_case "yields with no returns at all" `Quick
            yields_with_no_returns_at_all_is_dead;
          Alcotest.test_case "error position is consumed" `Quick
            error_position_is_consumed;
          Alcotest.test_case "two error positions" `Quick two_error_positions;
          Alcotest.test_case "returns without yields" `Quick
            returns_without_yields;
          Alcotest.test_case "returns builds wrong type" `Quick
            returns_wrong_type;
          Alcotest.test_case "returns ref unknown" `Quick returns_ref_unknown;
          Alcotest.test_case "error sentinel unknown" `Quick
            error_sentinel_unknown;
        ] );
      ( "foreign-boundary",
        [
          Alcotest.test_case "foreign struct as op output" `Quick
            foreign_struct_as_op_output;
          Alcotest.test_case "foreign struct as op input" `Quick
            foreign_struct_as_op_input;
          Alcotest.test_case "foreign struct as wire member" `Quick
            foreign_struct_as_wire_member;
          Alcotest.test_case "opaque handle as op output" `Quick
            opaque_handle_as_op_output;
          Alcotest.test_case "opaque handle as entry field ok" `Quick
            opaque_handle_as_entry_field_ok;
        ] );
      ( "closed-accounting",
        [
          Alcotest.test_case "split across two files, valid" `Quick
            split_valid_two_files;
          Alcotest.test_case "split within one file, valid" `Quick
            split_valid_within_one_file;
          Alcotest.test_case "conflicting module path" `Quick
            conflicting_module_path;
          Alcotest.test_case "duplicate extern name across files" `Quick
            duplicate_extern_name_across_files;
          Alcotest.test_case "lang block without module" `Quick
            lang_block_without_module;
        ] );
      ( "reference-example",
        [ Alcotest.test_case "compiles clean" `Quick reference_example_clean ]
      );
    ]
