open Tono_frontend

(* Typecheck coverage for an op's own "impl .field.method(args)" body
   (RFC-0023): the receiver must resolve to an entry field whose type is a
   declared opaque handle (TC0082), the method one of that handle's
   declared "extern" methods (TC0083), the argument count must match the
   method's declared logical parameters (TC0084), every argument ref must
   resolve like any other operation ref (TC0038), "impl" is rejected on a
   loose (non-entry) op (TC0044, mirroring the same rule for @header/@query/
   etc.), and it participates in the "exactly one implementation" count
   alongside @http and a legacy "ext impl" (TC0050/TC0051). Split out of
   [extern_typecheck_test.ml] to stay under the file-size cap; shares its
   [check]/[codes]/[has]/[parse_diag_count] helper shape. *)

let check src =
  let file, _ = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let _, tc = Typecheck.check_module ~file m in
  tc

let parse_diag_count src =
  let _, pdiags = Parser.parse src in
  List.length pdiags

let codes src = List.filter_map (fun (d : Diagnostic.t) -> d.code) (check src)
let has code src = List.mem code (codes src)

(* A bus with a two-arg "send" method, and a client with a "bus" handle
   field and an "endpoint" string field: shared base for every mutation
   below. *)
let bus_lib =
  {|ext bus {
  go: "github.com/x/bus"

  struct go_ack { OK: bool }

  type publisher {
    extern send(topic: string, body: string): ack {
      go {
        call: "Send"(topic, body)
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
|}

let op_impl_clean () =
  let src =
    bus_lib
    ^ {|
pub struct client {
  endpoint: string @arg
  bus: bus.publisher @with = bus.connect(.endpoint)

  op publish(topic: string): ack
    impl .bus.send(.topic, "body")
}
|}
  in
  Alcotest.(check int) "parses clean" 0 (parse_diag_count src);
  Alcotest.(check (list string)) "no codes" [] (codes src)

let op_impl_receiver_not_handle () =
  let src =
    bus_lib
    ^ {|
pub struct client {
  endpoint: string @arg

  op publish(topic: string): ack
    impl .endpoint.send(.topic, "body")
}
|}
  in
  Alcotest.(check bool) "receiver not a handle" true (has "TC0082" src)

let op_impl_receiver_unknown_field () =
  let src =
    bus_lib
    ^ {|
pub struct client {
  endpoint: string @arg
  bus: bus.publisher @with = bus.connect(.endpoint)

  op publish(topic: string): ack
    impl .bogus.send(.topic, "body")
}
|}
  in
  Alcotest.(check bool) "receiver unknown field" true (has "TC0038" src)

let op_impl_unknown_method () =
  let src =
    bus_lib
    ^ {|
pub struct client {
  endpoint: string @arg
  bus: bus.publisher @with = bus.connect(.endpoint)

  op publish(topic: string): ack
    impl .bus.publish(.topic, "body")
}
|}
  in
  Alcotest.(check bool) "unknown method" true (has "TC0083" src)

let op_impl_arity_mismatch () =
  let src =
    bus_lib
    ^ {|
pub struct client {
  endpoint: string @arg
  bus: bus.publisher @with = bus.connect(.endpoint)

  op publish(topic: string): ack
    impl .bus.send(.topic)
}
|}
  in
  Alcotest.(check bool) "arity mismatch" true (has "TC0084" src)

let op_impl_unknown_field_ref () =
  let src =
    bus_lib
    ^ {|
pub struct client {
  endpoint: string @arg
  bus: bus.publisher @with = bus.connect(.endpoint)

  op publish(topic: string): ack
    impl .bus.send(.bogus, "body")
}
|}
  in
  Alcotest.(check bool) "unknown field ref" true (has "TC0038" src)

let op_impl_on_loose_op_rejected () =
  let src =
    bus_lib
    ^ {|
op publish(topic: string): ack
  impl .bus.send(.topic, "body")
|}
  in
  Alcotest.(check bool) "impl on loose op rejected" true (has "TC0044" src)

let op_impl_conflicts_with_http () =
  let src =
    bus_lib
    ^ {|
pub struct client {
  endpoint: string @arg
  bus: bus.publisher @with = bus.connect(.endpoint)

  op publish(topic: string): ack
    @http(method: "POST", path: "/x", endpoint: .endpoint)
    impl .bus.send(.topic, "body")
}
|}
  in
  Alcotest.(check bool) "impl conflicts with http" true (has "TC0050" src)

let op_impl_bare_identifier_arg_rejected () =
  let src =
    bus_lib
    ^ {|
pub struct client {
  endpoint: string @arg
  bus: bus.publisher @with = bus.connect(.endpoint)

  op publish(topic: string): ack
    impl .bus.send(topic, "body")
}
|}
  in
  Alcotest.(check bool) "bare identifier arg rejected" true (has "TC0084" src)

let op_impl_arity_mismatch_plural () =
  let src =
    bus_lib
    ^ {|
pub struct client {
  endpoint: string @arg
  bus: bus.publisher @with = bus.connect(.endpoint)

  op publish(topic: string): ack
    impl .bus.send(.topic, "body", "extra")
}
|}
  in
  Alcotest.(check bool) "plural arity mismatch" true (has "TC0084" src)

let op_impl_receiver_is_own_param () =
  let src =
    bus_lib
    ^ {|
pub struct client {
  endpoint: string @arg

  op publish(bus: bus.publisher): ack
    impl .bus.send("t", "b")
}
|}
  in
  Alcotest.(check bool) "receiver is the op's own param" true (has "TC0082" src)

(* Every argument shape a ctor field value can carry in an "impl" call: a
   resolvable ref, a nested list of refs, a nested ctor, a "ns.fn(...)" call
   (which resolves inside its own ext block, not here), and a bare literal —
   all clean. *)
let op_impl_ctor_arg_recurses_every_shape_clean () =
  let src =
    bus_lib
    ^ {|
pub struct client {
  endpoint: string @arg
  bus: bus.publisher @with = bus.connect(.endpoint)

  op publish(topic: string): ack
    impl .bus.send(note {
      title: "x",
      ref: .topic,
      tags: [.topic, .topic],
      inner: other { title: .topic },
      via: bus.connect(.endpoint)
    }, "body")
}
|}
  in
  Alcotest.(check bool) "no unknown-field diagnostic" false (has "TC0038" src)

(* A ref buried in a nested list inside a ctor field still resolves like any
   other operation ref, so an unknown one is still caught. *)
let op_impl_ctor_arg_flags_unknown_nested_ref () =
  let src =
    bus_lib
    ^ {|
pub struct client {
  endpoint: string @arg
  bus: bus.publisher @with = bus.connect(.endpoint)

  op publish(topic: string): ack
    impl .bus.send(note { tags: [.topic, .bogus] }, "body")
}
|}
  in
  Alcotest.(check bool) "unknown nested ref" true (has "TC0038" src)

(* "impl" alone satisfies the "must have exactly one implementation" rule;
   no TC0051 (missing) fires. *)
let op_impl_satisfies_implementation_requirement () =
  let src =
    bus_lib
    ^ {|
pub struct client {
  endpoint: string @arg
  bus: bus.publisher @with = bus.connect(.endpoint)

  op publish(topic: string): ack
    impl .bus.send(.topic, "body")
}
|}
  in
  Alcotest.(check bool) "no missing-impl diagnostic" false (has "TC0051" src)

let () =
  Alcotest.run "op_impl"
    [
      ( "op-impl",
        [
          Alcotest.test_case "clean" `Quick op_impl_clean;
          Alcotest.test_case "receiver not a handle" `Quick
            op_impl_receiver_not_handle;
          Alcotest.test_case "receiver unknown field" `Quick
            op_impl_receiver_unknown_field;
          Alcotest.test_case "unknown method" `Quick op_impl_unknown_method;
          Alcotest.test_case "arity mismatch" `Quick op_impl_arity_mismatch;
          Alcotest.test_case "unknown field ref" `Quick
            op_impl_unknown_field_ref;
          Alcotest.test_case "rejected on a loose op" `Quick
            op_impl_on_loose_op_rejected;
          Alcotest.test_case "conflicts with @http" `Quick
            op_impl_conflicts_with_http;
          Alcotest.test_case "satisfies implementation requirement" `Quick
            op_impl_satisfies_implementation_requirement;
          Alcotest.test_case "bare identifier arg rejected" `Quick
            op_impl_bare_identifier_arg_rejected;
          Alcotest.test_case "plural arity mismatch" `Quick
            op_impl_arity_mismatch_plural;
          Alcotest.test_case "receiver is the op's own param" `Quick
            op_impl_receiver_is_own_param;
          Alcotest.test_case "ctor arg recurses every shape clean" `Quick
            op_impl_ctor_arg_recurses_every_shape_clean;
          Alcotest.test_case "ctor arg flags unknown nested ref" `Quick
            op_impl_ctor_arg_flags_unknown_nested_ref;
        ] );
    ]
