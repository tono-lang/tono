open Tono_frontend

(* End-to-end coverage for the "ext <name> { ... }" FFI library block: parse,
   format idempotency, lower, and IR JSON round-trip against the checked-in
   golden fixture (ir-schema/fixtures/extern_ffi.json). Unlike
   fmt_property_test.ml's synthetic AST samples, this drives the real source
   text through every stage the way a user's project would, which is the
   only thing that exercises Lower_extern and Ir_json_extern at all. *)

let src =
  {|import tono.http

ext companyconfig {
  go: "github.com/company/config"
  ts: "@company/config"

  struct go_config { Host: string, DevHost: string, Env: string, Credentials: go_creds }
  struct go_creds  { Secret: string }
  struct ts_config { host: string, token: string }
  struct ts_opts   { region: string, service: string }

  extern load(service: string, region: string): app_config {
    go {
      call: "Load"(service, region)
      yields: (cfg: go_config, err: error)
      returns: app_config {
        endpoint: match .cfg.Env { "prod" => .cfg.Host, _ => .cfg.DevHost }
        token: .cfg.Credentials.Secret
      }
    }
    ts {
      call: "load"(ts_opts {
        region: region,
        service: service,
        retries: 3,
        factor: 1.5,
        tags: [region, service],
        signer: companyauth.sign(.request),
        nested: ts_opts { region: region, service: service },
      })
      yields: (cfg: ts_config)
      returns: app_config { endpoint: .cfg.host, token: .cfg.token }
    }
  }
}

ext companybus {
  go: "github.com/company/bus"
  ts: "@company/bus"

  struct go_ack { ID: string, OK: bool }
  struct ts_ack { id: string, accepted: bool }

  type publisher {
    extern send(topic: string, body: string): ack {
      go {
        call: "Send"(topic, body)
        yields: (a: go_ack)
        returns: ack { id: .a.ID, accepted: .a.OK }
        errors: { "ErrBusy" => overloaded }
      }
      ts {
        call: "send"(topic, body)
        yields: (a: ts_ack)
        returns: ack { id: .a.id, accepted: .a.accepted }
        errors: { "BUSY" => overloaded }
      }
    }
  }

  extern connect(endpoint: string, token: string): publisher {
    go { call: "Connect"(endpoint, token) }
    ts { call: "connect"(endpoint, token) }
  }

  type ack_source("Source", ack) {
    extern get(): ack {
      go { call: "Get"() }
    }
  }
}

ext companyauth {
  go: "github.com/company/auth"
  ts: "@company/auth"

  extern sign(req: http.request): string {
    go { call: "Sign"(req) }
    ts { call: "sign"(req) }
  }
}

struct app_config { endpoint: string, token: string }
struct note_ref   { id: string }

pub struct note { id: string, body: string }
pub struct ack  { id: string, accepted: bool }

@doc("O barramento esta sobrecarregado.")
@retryable
pub struct overloaded { message: string }

@doc("Nenhuma nota com esse id.")
@status(404)
pub struct not_found { message: string }

@doc("SDK de notas: a config vem da lib interna, a publicacao passa pelo barramento.")
pub struct client {
  service: string @env("SERVICE_NAME") @default("notes")
  region: string @arg

  config: app_config = companyconfig.load(.service, .region)
  auth: string @format("Bearer {.config.token}")

  bus: companybus.publisher = companybus.connect(.config.endpoint, .config.token) @with

  op fetch(ref: note_ref): note
    @http(method: "GET", path: "/notes/{.ref.id}", endpoint: .config.endpoint)
    @header("Authorization", companyauth.sign(.request))
    @errors(not_found)
}
|}

let parse_clean () =
  let file, diags = Parser.parse src in
  Alcotest.(check int) "no parse diagnostics" 0 (List.length diags);
  let ext_libs =
    List.filter
      (fun (d : Ast.decl) ->
        match d.dkind with Ast.DExtLib _ -> true | _ -> false)
      file.decls
  in
  Alcotest.(check int) "three ext_lib decls" 3 (List.length ext_libs)

let fmt_idempotent () =
  let file, diags = Parser.parse src in
  Alcotest.(check int) "no parse diagnostics" 0 (List.length diags);
  let printed = Printer.print_file file in
  let reparsed, diags2 = Parser.parse printed in
  Alcotest.(check int) "no reparse diagnostics" 0 (List.length diags2);
  let printed2 = Printer.print_file reparsed in
  Alcotest.(check string) "idempotent" printed printed2

let lower_and_roundtrip () =
  let file, diags = Parser.parse src in
  Alcotest.(check int) "no parse diagnostics" 0 (List.length diags);
  let lower_diags = ref [] in
  let model = Lower.lower_file ~module_name:"notes" ~diags:lower_diags file in
  Alcotest.(check int) "no lower diagnostics" 0 (List.length !lower_diags);
  Alcotest.(check int) "three ext_libs" 3 (List.length model.ext_libs);
  let full : Ir.model =
    { tono_ir_version = Ir_json.current_ir_version; modules = [ model ] }
  in
  let json = Ir_json.encode_model full in
  match Ir_json.decode_model json with
  | Error e -> Alcotest.failf "decode failed: %s" e
  | Ok decoded ->
      let a = Ir_json.to_canonical_string json in
      let b = Ir_json.to_canonical_string (Ir_json.encode_model decoded) in
      Alcotest.(check string) "round-trip" a b

let () =
  Alcotest.run "extern"
    [
      ( "surface",
        [
          Alcotest.test_case "parses cleanly" `Quick parse_clean;
          Alcotest.test_case "fmt idempotent" `Quick fmt_idempotent;
        ] );
      ( "ir",
        [
          Alcotest.test_case "lowers and round-trips" `Quick lower_and_roundtrip;
        ] );
    ]
