open Tono_frontend

(* TC0093: the 'ctx' marker only applies to a foreign handle's own method
   call, never a free/library extern (including a field's own construction
   call). Split out of [extern_typecheck_test.ml] to stay under the
   file-size cap, the same reason [ext_lib_collisions_test.ml]/
   [op_impl_test.ml] have their own files. *)

let check src =
  let file, _ = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let _, tc = Typecheck.check_module ~file m in
  tc

let has code src =
  List.mem code
    (List.filter_map (fun (d : Diagnostic.t) -> d.code) (check src))

let ctx_marker_on_handle_method_ok () =
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
        ctx
      }
    }
  }

  extern connect(endpoint: string): publisher {
    go { call: "Connect"(endpoint) }
  }
}

struct ack { accepted: bool }
|}
  in
  Alcotest.(check bool) "no ctx diagnostic on a handle method" false
    (has "TC0093" src)

let ctx_marker_on_free_extern_rejected () =
  let src =
    {|ext lib {
  go: "github.com/x/y"

  struct go_cfg { Host: string }

  extern load(service: string): app_config {
    go {
      call: "Load"(service)
      yields: (cfg: go_cfg)
      returns: app_config { endpoint: .cfg.Host }
      ctx
    }
  }
}

struct app_config { endpoint: string }
|}
  in
  Alcotest.(check bool) "ctx on a free extern call is rejected" true
    (has "TC0093" src)

let () =
  Alcotest.run "ctx_marker"
    [
      ( "scope",
        [
          Alcotest.test_case "ctx marker on a handle method" `Quick
            ctx_marker_on_handle_method_ok;
          Alcotest.test_case "ctx marker on a free extern rejected" `Quick
            ctx_marker_on_free_extern_rejected;
        ] );
    ]
