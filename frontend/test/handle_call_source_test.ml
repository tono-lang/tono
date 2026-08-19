open Tono_frontend

(* A field whose value source is a call into a sibling opaque handle's
   method ("config: cfg = .provider.get()"): it parses next to the free-call
   source, lowers to the entry field's own "handle_call" (sharing the op
   impl's IR node) and round-trips through the JSON codec, the receiver and
   method resolve like an op's own "impl" call (TC0082/TC0083/TC0084/
   TC0038), the field's type must be the method's declared return
   (TC0094), the receiver is a dependency (TC0039 on a cycle), and the
   source-position/dead-source rules apply as they do to a free call
   (TC0035/TC0036). Split out of [op_impl_test.ml]/[entries_call_test.ml]
   to stay under the file-size cap. *)

let parse_and_lower src =
  let file, pdiags = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  (file, m, pdiags)

let check src =
  let file, m, _ = parse_and_lower src in
  let _, tc = Typecheck.check_module ~file m in
  tc

let parse_diag_count src =
  let _, pdiags = Parser.parse src in
  List.length pdiags

let codes src = List.filter_map (fun (d : Diagnostic.t) -> d.code) (check src)
let has code src = List.mem code (codes src)

let kit_lib =
  {|ext kit {
  go: "github.com/x/kit"

  struct go_cfg { ReadURL: string, WriteURL: string }

  type provider {
    extern get(): cfg {
      go {
        call: "Get"()
        yields: (c: go_cfg)
        returns: cfg { endpoint_read: .c.ReadURL, endpoint_write: .c.WriteURL }
        ctx
      }
    }
    extern get_for(region: string): cfg {
      go {
        call: "GetFor"(region)
        yields: (c: go_cfg)
        returns: cfg { endpoint_read: .c.ReadURL, endpoint_write: .c.WriteURL }
      }
    }
  }

  extern new_provider(name: string): provider {
    go { call: "NewProvider"(name) }
  }
}

struct cfg {
  endpoint_read: string
  endpoint_write: string
}

struct item { id: string }
|}

let entry_fields src =
  let _, m, _ = parse_and_lower src in
  List.concat_map
    (fun (s : Ir.shape) ->
      match s.kind with Ir.Entry { fields; _ } -> fields | _ -> [])
    m.Ir.shapes

let clean_source () =
  let src =
    kit_lib
    ^ {|
pub struct client {
  provider: kit.provider = kit.new_provider("kvs")
  config: cfg = .provider.get()

  op read(i: item): item @http(method: "GET", path: "/items/{.i.id}", endpoint: .config.endpoint_read)
  op write(i: item): item @http(method: "PUT", path: "/items", endpoint: .config.endpoint_write) @body(.i)
}
|}
  in
  Alcotest.(check int) "parses clean" 0 (parse_diag_count src);
  Alcotest.(check (list string)) "no codes" [] (codes src);
  let config =
    List.find
      (fun (f : Ir.entry_field) -> f.ef_name = "config")
      (entry_fields src)
  in
  Alcotest.(check bool) "no free call" true (Option.is_none config.ef_call);
  match config.ef_handle_call with
  | Some { Ir.oic_recv; oic_method; oic_args } ->
      Alcotest.(check (list string)) "receiver" [ "provider" ] oic_recv;
      Alcotest.(check string) "method" "get" oic_method;
      Alcotest.(check int) "no args" 0 (List.length oic_args)
  | None -> Alcotest.fail "the field lowers to a handle_call"

let with_marks_the_source_injectable_and_args_lower () =
  let src =
    kit_lib
    ^ {|
pub struct client {
  region: string @arg
  provider: kit.provider = kit.new_provider("kvs")
  scoped: cfg @with = .provider.get_for(.region)

  op read(i: item): item @http(method: "GET", path: "/items/{.i.id}", endpoint: .scoped.endpoint_read)
}
|}
  in
  Alcotest.(check (list string)) "no codes" [] (codes src);
  let scoped =
    List.find
      (fun (f : Ir.entry_field) -> f.ef_name = "scoped")
      (entry_fields src)
  in
  Alcotest.(check bool) "@with kept" true (List.mem Ir.With scoped.ef_sources);
  match scoped.ef_handle_call with
  | Some { Ir.oic_args = [ Ir.Ca_ref [ "region" ] ]; _ } -> ()
  | _ -> Alcotest.fail "the argument lowers to a field ref"

let ir_json_round_trips () =
  let src =
    kit_lib
    ^ {|
pub struct client {
  region: string @arg
  provider: kit.provider = kit.new_provider("kvs")
  scoped: cfg = .provider.get_for(.region)

  op read(i: item): item @http(method: "GET", path: "/items/{.i.id}", endpoint: .scoped.endpoint_read)
}
|}
  in
  let _, m, _ = parse_and_lower src in
  let model : Ir.model =
    { tono_ir_version = Ir_json.current_ir_version; modules = [ m ] }
  in
  let json = Ir_json.encode_model model in
  match Ir_json.decode_model json with
  | Ok decoded ->
      Alcotest.(check string)
        "encode . decode = id"
        (Ir_json.to_canonical_string json)
        (Ir_json.to_canonical_string (Ir_json.encode_model decoded))
  | Error e -> Alcotest.fail e

let fmt_round_trips () =
  let src =
    kit_lib
    ^ {|
pub struct client {
  region: string @arg
  provider: kit.provider = kit.new_provider("kvs")
  scoped: cfg = .provider.get_for(.region) @with
}
|}
  in
  let file, _ = Parser.parse src in
  let printed = Printer.print_file file in
  Alcotest.(check bool)
    "printer keeps the handle-call source" true
    (Option.is_some
       (Str.search_forward
          (Str.regexp_string "scoped: cfg = .provider.get_for(.region) @with")
          printed 0
       |> Option.some));
  let file', pdiags = Parser.parse printed in
  Alcotest.(check int) "reparses clean" 0 (List.length pdiags);
  Alcotest.(check string) "stable" printed (Printer.print_file file')

let receiver_not_a_handle () =
  let src =
    kit_lib
    ^ {|
pub struct client {
  name: string @arg
  config: cfg = .name.get()
  op read(i: item): item @http(method: "GET", path: "/items/{.i.id}", endpoint: "https://x.test")
}
|}
  in
  Alcotest.(check bool) "TC0082" true (has "TC0082" src)

let unknown_receiver () =
  let src =
    kit_lib
    ^ {|
pub struct client {
  config: cfg = .provider.get()
  op read(i: item): item @http(method: "GET", path: "/items/{.i.id}", endpoint: "https://x.test")
}
|}
  in
  Alcotest.(check bool) "TC0038" true (has "TC0038" src)

let unknown_method () =
  let src =
    kit_lib
    ^ {|
pub struct client {
  provider: kit.provider = kit.new_provider("kvs")
  config: cfg = .provider.fetch()
  op read(i: item): item @http(method: "GET", path: "/items/{.i.id}", endpoint: "https://x.test")
}
|}
  in
  Alcotest.(check bool) "TC0083" true (has "TC0083" src)

let arity_mismatch () =
  let src =
    kit_lib
    ^ {|
pub struct client {
  provider: kit.provider = kit.new_provider("kvs")
  config: cfg = .provider.get("extra")
  op read(i: item): item @http(method: "GET", path: "/items/{.i.id}", endpoint: "https://x.test")
}
|}
  in
  Alcotest.(check bool) "TC0084" true (has "TC0084" src)

let unknown_argument_ref () =
  let src =
    kit_lib
    ^ {|
pub struct client {
  provider: kit.provider = kit.new_provider("kvs")
  config: cfg = .provider.get_for(.region)
  op read(i: item): item @http(method: "GET", path: "/items/{.i.id}", endpoint: "https://x.test")
}
|}
  in
  Alcotest.(check bool) "TC0038" true (has "TC0038" src)

let request_argument_rejected () =
  let src =
    kit_lib
    ^ {|
pub struct client {
  provider: kit.provider = kit.new_provider("kvs")
  config: cfg = .provider.get_for(.request)
  op read(i: item): item @http(method: "GET", path: "/items/{.i.id}", endpoint: "https://x.test")
}
|}
  in
  Alcotest.(check bool) "TC0085" true (has "TC0085" src)

let field_type_must_be_the_method_return () =
  let src =
    kit_lib
    ^ {|
pub struct client {
  provider: kit.provider = kit.new_provider("kvs")
  config: string = .provider.get()
  op read(i: item): item @http(method: "GET", path: "/items/{.i.id}", endpoint: "https://x.test")
}
|}
  in
  Alcotest.(check bool) "TC0094" true (has "TC0094" src)

let receiver_is_a_dependency_so_a_cycle_is_diagnosed () =
  let src =
    kit_lib
    ^ {|
pub struct client {
  provider: kit.provider = kit.new_provider(.config.endpoint_read)
  config: cfg = .provider.get()
  op read(i: item): item @http(method: "GET", path: "/items/{.i.id}", endpoint: "https://x.test")
}
|}
  in
  Alcotest.(check bool) "TC0039" true (has "TC0039" src)

let competing_source_is_dead () =
  let src =
    kit_lib
    ^ {|
pub struct client {
  provider: kit.provider = kit.new_provider("kvs")
  config: cfg @env("CFG") = .provider.get()
  op read(i: item): item @http(method: "GET", path: "/items/{.i.id}", endpoint: "https://x.test")
}
|}
  in
  Alcotest.(check bool) "TC0036" true (has "TC0036" src)

let rejected_on_a_wire_struct () =
  let src =
    kit_lib ^ {|
struct payload {
  config: cfg = .provider.get()
}
|}
  in
  Alcotest.(check bool) "TC0035" true (has "TC0035" src)

let config_has_no_handle_to_call () =
  let src =
    kit_lib
    ^ {|
struct settings {
  name: string @env("NAME")
  config: cfg = .name.get()
}

pub struct client {
  settings: settings
}
|}
  in
  Alcotest.(check bool) "TC0082" true (has "TC0082" src)

let parse_errors_name_the_form () =
  let missing_receiver =
    kit_lib
    ^ {|
pub struct client {
  config: cfg = .get()
  op read(i: item): item @http(method: "GET", path: "/items/{.i.id}", endpoint: "https://x.test")
}
|}
  in
  Alcotest.(check bool)
    "receiver required" true
    (parse_diag_count missing_receiver > 0);
  let missing_parens =
    kit_lib
    ^ {|
pub struct client {
  provider: kit.provider = kit.new_provider("kvs")
  config: cfg = .provider.get
  op read(i: item): item @http(method: "GET", path: "/items/{.i.id}", endpoint: "https://x.test")
}
|}
  in
  Alcotest.(check bool)
    "parens required" true
    (parse_diag_count missing_parens > 0);
  let stray_token =
    kit_lib
    ^ {|
pub struct client {
  config: cfg = 42
  op read(i: item): item @http(method: "GET", path: "/items/{.i.id}", endpoint: "https://x.test")
}
|}
  in
  Alcotest.(check bool)
    "value form named" true
    (parse_diag_count stray_token > 0)

let () =
  Alcotest.run "handle_call_source"
    [
      ( "field = .handle.method(args)",
        [
          Alcotest.test_case "clean source lowers to handle_call" `Quick
            clean_source;
          Alcotest.test_case "@with and args" `Quick
            with_marks_the_source_injectable_and_args_lower;
          Alcotest.test_case "IR JSON round-trips" `Quick ir_json_round_trips;
          Alcotest.test_case "fmt round-trips" `Quick fmt_round_trips;
          Alcotest.test_case "receiver not a handle" `Quick
            receiver_not_a_handle;
          Alcotest.test_case "unknown receiver" `Quick unknown_receiver;
          Alcotest.test_case "unknown method" `Quick unknown_method;
          Alcotest.test_case "arity mismatch" `Quick arity_mismatch;
          Alcotest.test_case "unknown argument ref" `Quick unknown_argument_ref;
          Alcotest.test_case ".request argument rejected" `Quick
            request_argument_rejected;
          Alcotest.test_case "field type is the method return" `Quick
            field_type_must_be_the_method_return;
          Alcotest.test_case "receiver cycle" `Quick
            receiver_is_a_dependency_so_a_cycle_is_diagnosed;
          Alcotest.test_case "competing source is dead" `Quick
            competing_source_is_dead;
          Alcotest.test_case "rejected on a wire struct" `Quick
            rejected_on_a_wire_struct;
          Alcotest.test_case "config has no handle" `Quick
            config_has_no_handle_to_call;
          Alcotest.test_case "parse errors" `Quick parse_errors_name_the_form;
        ] );
    ]
