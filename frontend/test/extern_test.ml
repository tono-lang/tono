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
  go { #(github.com/company/config) }
  ts { #(@company/config) }

  struct go_config { Host: string, DevHost: string, Env: string, Credentials: go_creds }
  struct go_creds  { Secret: string }
  struct ts_config { host: string, token: string }
  struct ts_opts   { region: string, service: string }

  op load(service: string, region: string): app_config {
    go {
      call: #(Load)(service, #(Region)(region))
      yields: (cfg: go_config, err: error)
      returns: app_config {
        endpoint: match .cfg.Env { "prod" => .cfg.Host, _ => .cfg.DevHost }
        token: .cfg.Credentials.Secret
      }
    }
    ts {
      call: #(load)(ts_opts {
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

  op with_precision(digits: i64): app_config {
    go { call: #(WithPrecision)(digits) }
  }

  op build(seed: string, opts: []app_config): app_config {
    go { call: #(Build)(seed, opts) }
    ts { call: #(new Build)(seed, opts) }
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

  struct ack_source {

    go { #(Source[Ack]) }

    ts { #(Source<Ack>) }
    op get(): ack {
      go { call: #(Get)() }
    }

    op latest(key: string): ack {
      go {
        call: #(Fetch)(#(ctx context.Context), key).#(Result)()
        yields: (a: go_ack)
        returns: ack { id: .a.ID, accepted: .a.OK }
      }
    }
  }
}

ext companyauth {
  go { #(github.com/company/auth) }
  ts { #(@company/auth) }

  op sign(req: http.request): string {
    go { call: #(Sign)(req) }
    ts { call: #(sign)(req) }
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
  precision_opt: app_config = companyconfig.with_precision(3)
  built: app_config = companyconfig.build(.service, [.precision_opt])
  auth: string @format("Bearer {.config.token}")

  bus: companybus.publisher = companybus.connect(.config.endpoint, .config.token) @with

  @http(method: "GET", path: "/notes/{.ref.id}", endpoint: .config.endpoint)
  @header("Authorization", companyauth.sign(.request))
  @errors(not_found)
  op fetch(ref: note_ref): note
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

(* The method chained on the returned object lowers into its own slot of
   the binding, as the symbol-call node a nested argument already uses, and
   never into the callee spelling or the argument list. *)
let chain_lowers () =
  let file, _ = Parser.parse src in
  let model = Lower.lower_file ~module_name:"notes" ~diags:(ref []) file in
  let bus =
    List.find (fun (l : Ir.ext_lib) -> l.xl_name = "companybus") model.ext_libs
  in
  let source =
    List.find
      (fun (t : Ir.opaque_type) -> t.opq_name = "ack_source")
      bus.xl_types
  in
  let latest =
    List.find
      (fun (m : Ir.extern_decl) -> m.x_name = "latest")
      source.opq_methods
  in
  let go = List.hd latest.x_langs in
  Alcotest.(check string) "callee stays the first call" "Fetch" go.el_symbol;
  Alcotest.(check int)
    "the first call keeps its arguments" 2
    (List.length go.el_call_args);
  match go.el_chain with
  | Some { Ir.scl_symbol = "Result"; scl_args = [] } -> ()
  | _ -> Alcotest.fail "expected the chain Result() on the binding"

(* The checked-in golden fixture is this very source, lowered: the backend's
   round-trip test reads it as the frontend's encoding of the ext surface.
   Set TONO_WRITE_FIXTURES=1 to rewrite it after an IR change. *)
let golden_path = "../../../../ir-schema/fixtures/extern_ffi.json"

let golden_fixture_matches () =
  let file, _ = Parser.parse src in
  let model = Lower.lower_file ~module_name:"notes" ~diags:(ref []) file in
  let full : Ir.model =
    { tono_ir_version = Ir_json.current_ir_version; modules = [ model ] }
  in
  let encoded = Ir_json.to_canonical_string (Ir_json.encode_model full) in
  if Sys.getenv_opt "TONO_WRITE_FIXTURES" = Some "1" then
    Out_channel.with_open_bin golden_path (fun oc ->
        output_string oc encoded;
        output_char oc '\n');
  let on_disk =
    In_channel.with_open_bin golden_path In_channel.input_all |> String.trim
  in
  Alcotest.(check string)
    "ir-schema/fixtures/extern_ffi.json is current" encoded on_disk

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
          Alcotest.test_case "chain lowers into its own slot" `Quick
            chain_lowers;
          Alcotest.test_case "golden fixture is current" `Quick
            golden_fixture_matches;
        ] );
    ]
