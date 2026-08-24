open Tono_frontend

let src =
  {|ext gearbox {
  go { #(example.test/gearbox) }
  ts { #(@example/gearbox) }

  struct dial_options {
    precision: u8
    ts { #(DialOptions) }
  }

  struct dial {
    go { #(Dial[float64]) }
    ts { #(Dial<number>) }

    op read(): float {
      go { call: #(Read)(#(ctx context.Context)) }
      ts { call: #(read)() }
    }
  }

  op open(name: string, precision: u8): dial {
    go { call: #(Open[float64])(name, precision: #(int)) }
    ts { call: #(new Dial)(name, dial_options { precision: precision }) }
  }

  op tune(name: string): dial {
    go { call: #(Tune[float64])(name, #(WithPrecision)(4), "fine") }
    ts { call: #(tune)([1, 2], .name) }
  }

  op fetch(key: string): string {
    go { call: #(Fetch)(#(ctx context.Context), key).#(Result)() }
  }
}
|}

let sites () =
  let file, diags = Parser.parse src in
  if diags <> [] then
    Alcotest.failf "unexpected parse diagnostics: %s"
      (String.concat "; " (List.map Diagnostic.to_string diags));
  Ext_sites.of_file file

let find kind ?owner ?name lang =
  List.find_opt
    (fun (s : Ext_sites.site) ->
      s.kind = kind && s.lang = lang && s.owner = owner && s.name = name)
    (sites ())

let span s =
  match s with
  | Some (s : Ext_sites.site) -> Span.to_string s.span
  | None -> "<missing>"

let every_binding_has_a_site () =
  Alcotest.(check int) "site count" 12 (List.length (sites ()))

(* The method chained on the returned object is part of the call: line, so
   a finding about the binding covers it. *)
let chained_method_extends_the_call_line () =
  Alcotest.(check string)
    "go op call with a chain" "31:16-63"
    (span (find Ext_sites.Op ~name:"fetch" "go"))

let path_site_points_at_the_spelling () =
  Alcotest.(check string) "go path" "2:8-31" (span (find Ext_sites.Path "go"))

let struct_site_spans_the_block () =
  Alcotest.(check string)
    "ts form" "7:5-26"
    (span (find Ext_sites.Struct ~name:"dial_options" "ts"))

let handle_site_spans_the_block () =
  Alcotest.(check string)
    "go storage" "11:5-28"
    (span (find Ext_sites.Handle ~name:"dial" "go"))

let method_site_spans_the_call_line () =
  Alcotest.(check string)
    "go method call" "15:18-48"
    (span (find Ext_sites.Method ~owner:"dial" ~name:"read" "go"));
  (* No argument: the callee spelling alone. *)
  Alcotest.(check string)
    "ts method call" "16:18-25"
    (span (find Ext_sites.Method ~owner:"dial" ~name:"read" "ts"))

let op_site_spans_the_call_line () =
  Alcotest.(check string)
    "go op call" "21:16-56"
    (span (find Ext_sites.Op ~name:"open" "go"));
  Alcotest.(check string)
    "ts op call (ctor argument last)" "22:16-71"
    (span (find Ext_sites.Op ~name:"open" "ts"))

let nested_call_and_literal_extend_the_call_line () =
  Alcotest.(check string)
    "go op call with a nested call and a literal" "26:16-66"
    (span (find Ext_sites.Op ~name:"tune" "go"));
  Alcotest.(check string)
    "ts op call with a list and a reference" "27:16-37"
    (span (find Ext_sites.Op ~name:"tune" "ts"))

let json_shape () =
  match find Ext_sites.Method ~owner:"dial" ~name:"read" "go" with
  | None -> Alcotest.fail "site missing"
  | Some s ->
      Alcotest.(check string)
        "json"
        {|{"ext":"gearbox","lang":"go","kind":"method","owner":"dial","name":"read","span":"15:18-48"}|}
        (Yojson.Safe.to_string (Ext_sites.to_json s))

let kinds_render () =
  Alcotest.(check (list string))
    "kinds"
    [ "path"; "handle"; "struct"; "op"; "method" ]
    (List.map Ext_sites.kind_to_string
       Ext_sites.[ Path; Handle; Struct; Op; Method ])

let cli_lists_one_object_per_line () =
  let o =
    Cli.run ~read_file:(fun _ -> src) [| "x"; "ext-bindings"; "g.tono" |]
  in
  Alcotest.(check int) "exit" 0 o.code;
  let lines = String.split_on_char '\n' (String.trim o.out) in
  Alcotest.(check int) "lines" 12 (List.length lines);
  Alcotest.(check bool)
    "first is the go path" true
    (String.length (List.hd lines) > 0
    && List.hd lines
       = {|{"ext":"gearbox","lang":"go","kind":"path","owner":null,"name":null,"span":"2:8-31"}|}
    )

let cli_rejects_a_source_that_does_not_parse () =
  let o =
    Cli.run ~read_file:(fun _ -> "ext {") [| "x"; "ext-bindings"; "g.tono" |]
  in
  Alcotest.(check int) "exit" 1 o.code;
  Alcotest.(check bool) "diagnostics on stderr" true (o.err <> "");
  Alcotest.(check string) "nothing on stdout" "" o.out

let cli_without_a_path_is_usage () =
  let o = Cli.run ~read_file:(fun _ -> src) [| "x"; "ext-bindings" |] in
  Alcotest.(check int) "exit" 2 o.code

let cli_unreadable_file () =
  let o =
    Cli.run
      ~read_file:(fun _ -> raise (Sys_error "nope"))
      [| "x"; "ext-bindings"; "g.tono" |]
  in
  Alcotest.(check int) "exit" 1 o.code;
  Alcotest.(check string) "message" "nope\n" o.err

let () =
  Alcotest.run "ext_sites"
    [
      ( "sites",
        [
          Alcotest.test_case "every binding has a site" `Quick
            every_binding_has_a_site;
          Alcotest.test_case "path site" `Quick path_site_points_at_the_spelling;
          Alcotest.test_case "struct site" `Quick struct_site_spans_the_block;
          Alcotest.test_case "handle site" `Quick handle_site_spans_the_block;
          Alcotest.test_case "method site" `Quick
            method_site_spans_the_call_line;
          Alcotest.test_case "op site" `Quick op_site_spans_the_call_line;
          Alcotest.test_case "chained method" `Quick
            chained_method_extends_the_call_line;
          Alcotest.test_case "nested call and literal" `Quick
            nested_call_and_literal_extend_the_call_line;
          Alcotest.test_case "json" `Quick json_shape;
          Alcotest.test_case "kinds" `Quick kinds_render;
        ] );
      ( "cli",
        [
          Alcotest.test_case "one object per line" `Quick
            cli_lists_one_object_per_line;
          Alcotest.test_case "parse error" `Quick
            cli_rejects_a_source_that_does_not_parse;
          Alcotest.test_case "usage" `Quick cli_without_a_path_is_usage;
          Alcotest.test_case "unreadable" `Quick cli_unreadable_file;
        ] );
    ]
