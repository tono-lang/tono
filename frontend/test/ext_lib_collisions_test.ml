open Tono_frontend

(* Coverage split out of [extern_typecheck_test.ml] to stay under the
   file-size cap: module-local foreign-name collisions (TC0080's in-file
   half — the cross-file half lives in [extern_typecheck_test.ml]'s
   "closed-accounting" suite), an opaque handle applied as if it were
   generic (TC0005), and a logical parameter forwarded only through a nested
   ctor/list argument (proving [Check_ext_lib.collect_call_arg]/
   [collect_trait_arg] walk every nesting shape, not just a bare call: arg).
   Shares [extern_typecheck_test.ml]'s [check]/[codes]/[has] helper shape. *)

let check src =
  let file, _ = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let _, tc = Typecheck.check_module ~file m in
  tc

let codes src = List.filter_map (fun (d : Diagnostic.t) -> d.code) (check src)
let has code src = List.mem code (codes src)

(* ── In-module foreign-name collisions (single file, no project needed) ─── *)

(* A foreign struct name that shadows an already-declared shape name: the
   module-local half of TC0080. *)
let foreign_struct_collides_with_shape () =
  let src =
    {|struct shared_name { note: string }

ext lib {
  go: "github.com/x/y"

  struct shared_name { Host: string }

  extern load(service: string): app_config {
    go {
      call: "Load"(service)
      yields: (cfg: shared_name)
      returns: app_config { endpoint: .cfg.Host }
    }
  }
}

struct app_config { endpoint: string }
|}
  in
  Alcotest.(check bool) "collides with a shape name" true (has "TC0080" src)

(* Two "ext lib" blocks in the same file, both declaring a struct under the
   same name: the same account the cross-file check closes, but within one
   file. *)
let foreign_struct_duplicated_within_one_file () =
  let src =
    {|ext lib {
  go: "github.com/x/y"

  struct dup_thing { Host: string }

  extern load(service: string): app_config {
    go {
      call: "Load"(service)
      yields: (cfg: dup_thing)
      returns: app_config { endpoint: .cfg.Host }
    }
  }
}

ext lib {
  struct dup_thing { Other: string }
}

struct app_config { endpoint: string }
|}
  in
  Alcotest.(check bool) "duplicated within one file" true (has "TC0080" src)

(* ── An opaque handle is not generic; applying type args is TC0005 ──────── *)

let opaque_handle_applied_as_generic () =
  let src =
    {|ext bus {
  go: "github.com/x/bus"

  struct go_ack { OK: bool }

  type publisher {
    extern send(topic: string): ack {
      go {
        call: "Send"(topic)
        yields: (a: go_ack)
        returns: ack { accepted: .a.OK }
      }
    }
  }

  extern connect(endpoint: string): publisher {
    go { call: "Connect"(endpoint) }
  }
}

pub struct ack { accepted: bool }
struct holder { b: bus.publisher[i64] }
|}
  in
  Alcotest.(check bool) "opaque handle is not generic" true (has "TC0005" src)

(* A qualified reference whose qualifier names an ordinary decl (not an
   "ext" block) never matches [qualified_of]'s opaque-type scan -- that scan
   walks every decl but only [DExtLib] ones can carry opaque types. Falls
   through to the generic qualified-reference fallback (an unknown import,
   with no local "ext"/import map in scope). *)
let qualifier_names_a_non_ext_decl_falls_through () =
  let src =
    {|struct notabus { x: string }
struct holder { p: notabus.thing }
|}
  in
  Alcotest.(check bool)
    "falls through to unknown import" true (has "TC0023" src)

(* A bare (unqualified) reference to a foreign struct name is also not
   generic; [Resolve.resolve_head]'s [known] branch covers this even without
   going through the "ext" qualifier path [opaque_handle_applied_as_generic]
   exercises. *)
let bare_foreign_name_applied_as_generic () =
  let src =
    {|ext lib {
  go: "github.com/x/y"

  struct go_cfg { Host: string }

  extern load(service: string): app_config {
    go {
      call: "Load"(service)
      yields: (cfg: go_cfg)
      returns: app_config { endpoint: .cfg.Host }
    }
  }
}

struct app_config { endpoint: string }
struct holder { c: go_cfg[i64] }
|}
  in
  Alcotest.(check bool)
    "bare foreign name is not generic" true (has "TC0005" src)

(* ── Nested call: shapes: a list/ctor of refs still counts as consumption ─ *)

(* A parameter forwarded only inside a nested ctor/list argument (not a bare
   call: arg) still counts as consumed; walking [collect_call_arg]/
   [collect_trait_arg] through every nesting shape is what proves it. *)
let param_consumed_through_nested_ctor_and_list () =
  let src =
    {|ext lib {
  go: "github.com/x/y"

  struct go_cfg { Host: string }
  struct go_opts { Names: []string }

  extern load(service: string, region: string): app_config {
    go {
      call: "Load"(go_opts { Names: [service, region] })
      yields: (cfg: go_cfg)
      returns: app_config { endpoint: .cfg.Host }
    }
  }
}

struct app_config { endpoint: string }
|}
  in
  Alcotest.(check (list string))
    "no unconsumed-param codes" []
    (List.filter (String.equal "TC0078") (codes src))

let () =
  Alcotest.run "ext_lib_collisions"
    [
      ( "foreign-name-collisions",
        [
          Alcotest.test_case "collides with shape" `Quick
            foreign_struct_collides_with_shape;
          Alcotest.test_case "duplicated within one file" `Quick
            foreign_struct_duplicated_within_one_file;
        ] );
      ( "opaque-generics",
        [
          Alcotest.test_case "opaque handle applied as generic" `Quick
            opaque_handle_applied_as_generic;
          Alcotest.test_case "bare foreign name applied as generic" `Quick
            bare_foreign_name_applied_as_generic;
          Alcotest.test_case "qualifier names a non-ext decl" `Quick
            qualifier_names_a_non_ext_decl_falls_through;
        ] );
      ( "call-nesting",
        [
          Alcotest.test_case "param consumed through nested ctor and list"
            `Quick param_consumed_through_nested_ctor_and_list;
        ] );
    ]
