open Tono_frontend

(* A minimal source that lowers and typechecks cleanly. *)
let demo_src = "struct point { x: i64\n y: i64 }"

(* Substring test, kept local so the suite needs no extra dependency. *)
let contains s sub =
  let ls = String.length s and lsub = String.length sub in
  let rec go i = i + lsub <= ls && (String.sub s i lsub = sub || go (i + 1)) in
  lsub = 0 || go 0

let compile_ok () =
  match compile_to_json ~module_name:"geo" demo_src with
  | Ok json ->
      Alcotest.(check bool) "carries the module name" true (contains json "geo");
      Alcotest.(check bool)
        "carries the ir version" true
        (contains json "tono_ir_version")
  | Error msg -> Alcotest.failf "expected Ok, got: %s" msg

let compile_error () =
  match compile_to_json ~module_name:"geo" "struct {" with
  | Ok _ -> Alcotest.fail "expected an error for malformed source"
  | Error msg -> Alcotest.(check bool) "non-empty message" true (msg <> "")

let compile_default_module () =
  (* Omitting [module_name] uses the empty default and still encodes. *)
  match compile_to_json demo_src with
  | Ok json ->
      Alcotest.(check bool)
        "still encodes" true
        (contains json "tono_ir_version")
  | Error msg -> Alcotest.failf "expected Ok, got: %s" msg

let always src _ = src

let run_compile_basename () =
  let o =
    Cli.run ~read_file:(always demo_src) [| "x"; "compile"; "demo.tono" |]
  in
  Alcotest.(check int) "exit 0" 0 o.Cli.code;
  Alcotest.(check string) "no stderr" "" o.Cli.err;
  Alcotest.(check bool)
    "module name defaults to the basename" true
    (contains o.Cli.out "demo")

let run_compile_module_flag () =
  let o =
    Cli.run ~read_file:(always demo_src)
      [| "x"; "compile"; "demo.tono"; "--module"; "billing" |]
  in
  Alcotest.(check int) "exit 0" 0 o.Cli.code;
  Alcotest.(check bool)
    "the --module flag overrides the name" true
    (contains o.Cli.out "billing")

let run_missing_path () =
  let o = Cli.run ~read_file:(always demo_src) [| "x"; "compile" |] in
  Alcotest.(check int) "usage exit code" 2 o.Cli.code;
  Alcotest.(check bool) "usage on stderr" true (contains o.Cli.err "usage")

let run_file_not_found () =
  let raising p = raise (Sys_error (p ^ ": No such file")) in
  let o = Cli.run ~read_file:raising [| "x"; "compile"; "nope.tono" |] in
  Alcotest.(check int) "io error exit code" 1 o.Cli.code;
  Alcotest.(check bool)
    "reports the io error" true
    (contains o.Cli.err "nope.tono")

let run_compile_invalid_source () =
  let o =
    Cli.run ~read_file:(always "struct {") [| "x"; "compile"; "bad.tono" |]
  in
  Alcotest.(check int) "compile error exit code" 1 o.Cli.code;
  Alcotest.(check bool) "diagnostic on stderr" true (o.Cli.err <> "")

let run_version () =
  let o = Cli.run ~read_file:(always "") [| "x"; "version" |] in
  Alcotest.(check int) "exit 0" 0 o.Cli.code;
  Alcotest.(check bool) "prints the version" true (contains o.Cli.out "tono ")

let run_no_args () =
  let o = Cli.run ~read_file:(always "") [| "x" |] in
  Alcotest.(check int) "bare invocation prints version" 0 o.Cli.code

let run_unknown () =
  let o = Cli.run ~read_file:(always "") [| "x"; "wat" |] in
  Alcotest.(check int) "unknown command is a usage error" 2 o.Cli.code

let run_extra_bare_arg () =
  (* A second bare argument after the path is ignored; the first one wins. *)
  let o =
    Cli.run ~read_file:(always demo_src)
      [| "x"; "compile"; "demo.tono"; "extra.tono" |]
  in
  Alcotest.(check int) "exit 0" 0 o.Cli.code;
  Alcotest.(check bool)
    "the first path's basename wins" true
    (contains o.Cli.out "demo")

let () =
  Alcotest.run "cli"
    [
      ( "compile_to_json",
        [
          Alcotest.test_case "happy path" `Quick compile_ok;
          Alcotest.test_case "errors abort" `Quick compile_error;
          Alcotest.test_case "default module" `Quick compile_default_module;
        ] );
      ( "run",
        [
          Alcotest.test_case "basename module" `Quick run_compile_basename;
          Alcotest.test_case "module flag" `Quick run_compile_module_flag;
          Alcotest.test_case "missing path" `Quick run_missing_path;
          Alcotest.test_case "file not found" `Quick run_file_not_found;
          Alcotest.test_case "invalid source" `Quick run_compile_invalid_source;
          Alcotest.test_case "version" `Quick run_version;
          Alcotest.test_case "no args" `Quick run_no_args;
          Alcotest.test_case "unknown command" `Quick run_unknown;
          Alcotest.test_case "extra bare arg" `Quick run_extra_bare_arg;
        ] );
    ]
