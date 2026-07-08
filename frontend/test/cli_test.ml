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

let run_fmt () =
  let o =
    Cli.run
      ~read_file:(always "struct point { x: i64, y: i64 }")
      [| "x"; "fmt"; "demo.tono" |]
  in
  Alcotest.(check int) "exit 0" 0 o.Cli.code;
  Alcotest.(check string) "no stderr" "" o.Cli.err;
  Alcotest.(check string)
    "canonical form on stdout" "struct point {\n  x: i64\n  y: i64\n}\n"
    o.Cli.out

let run_fmt_invalid_source () =
  let o = Cli.run ~read_file:(always "struct {") [| "x"; "fmt"; "bad.tono" |] in
  Alcotest.(check int) "parse error exit code" 1 o.Cli.code;
  Alcotest.(check string) "no stdout" "" o.Cli.out;
  Alcotest.(check bool) "diagnostic on stderr" true (o.Cli.err <> "")

let run_fmt_missing_path () =
  let o = Cli.run ~read_file:(always "") [| "x"; "fmt" |] in
  Alcotest.(check int) "usage exit code" 2 o.Cli.code;
  Alcotest.(check bool) "usage on stderr" true (contains o.Cli.err "usage")

let run_fmt_file_not_found () =
  let raising p = raise (Sys_error (p ^ ": No such file")) in
  let o = Cli.run ~read_file:raising [| "x"; "fmt"; "nope.tono" |] in
  Alcotest.(check int) "io error exit code" 1 o.Cli.code;
  Alcotest.(check bool)
    "reports the io error" true
    (contains o.Cli.err "nope.tono")

(* Drive [compile-dir] with an in-memory project: [listing] are the paths a
   walker would return relative to the root, [files] maps each full path to its
   source. *)
let dir_run ~files ~listing argv =
  let list_files _ = listing in
  let read_file path =
    match List.assoc_opt path files with
    | Some s -> s
    | None -> raise (Sys_error (path ^ ": No such file"))
  in
  Cli.run ~list_files ~read_file argv

let proj_files root =
  [
    (root ^ "/payments/common.tono", "pub struct money { amount: i64 }");
    ( root ^ "/payments/charge.tono",
      "import payments.common\n\npub struct charge { total: common.money }" );
  ]

let run_compile_dir_ok () =
  let root = "/proj" in
  let o =
    dir_run ~files:(proj_files root)
      ~listing:[ "payments/charge.tono"; "payments/common.tono" ]
      [| "x"; "compile-dir"; root |]
  in
  Alcotest.(check int) "exit 0" 0 o.Cli.code;
  Alcotest.(check string) "no stderr" "" o.Cli.err;
  Alcotest.(check bool)
    "derives dotted module names" true
    (contains o.Cli.out "payments.common");
  Alcotest.(check bool)
    "cross-module reference is qualified" true
    (contains o.Cli.out "payments.common#money")

let run_compile_dir_visibility () =
  let root = "/proj" in
  let files =
    [
      (root ^ "/payments/common.tono", "struct money { amount: i64 }");
      (* not pub *)
      ( root ^ "/payments/charge.tono",
        "import payments.common\n\npub struct charge { total: common.money }" );
    ]
  in
  let o =
    dir_run ~files
      ~listing:[ "payments/charge.tono"; "payments/common.tono" ]
      [| "x"; "compile-dir"; root |]
  in
  Alcotest.(check int) "visibility violation fails" 1 o.Cli.code;
  Alcotest.(check bool)
    "reports the non-pub reference" true
    (contains o.Cli.err "not exported")

let run_compile_dir_cycle () =
  let root = "/proj" in
  let files =
    [
      (root ^ "/a.tono", "import b\n\npub struct ta { x: i64 }");
      (root ^ "/b.tono", "import a\n\npub struct tb { x: i64 }");
    ]
  in
  let o =
    dir_run ~files ~listing:[ "a.tono"; "b.tono" ]
      [| "x"; "compile-dir"; root |]
  in
  Alcotest.(check int) "cycle fails" 1 o.Cli.code;
  Alcotest.(check bool) "reports the cycle" true (contains o.Cli.err "cycle")

let run_compile_dir_empty () =
  let o =
    dir_run ~files:[] ~listing:[ "readme.md" ] [| "x"; "compile-dir"; "/proj" |]
  in
  Alcotest.(check int) "no sources is an error" 1 o.Cli.code;
  Alcotest.(check bool)
    "explains the empty project" true
    (contains o.Cli.err "no .tono")

let run_compile_dir_missing_root () =
  let list_files _ = raise (Sys_error "/nope: No such file or directory") in
  let o =
    Cli.run ~list_files ~read_file:(always "") [| "x"; "compile-dir"; "/nope" |]
  in
  Alcotest.(check int) "io error exit code" 1 o.Cli.code;
  Alcotest.(check bool) "reports the io error" true (contains o.Cli.err "/nope")

let run_compile_dir_missing_arg () =
  let o = Cli.run ~read_file:(always "") [| "x"; "compile-dir" |] in
  Alcotest.(check int) "usage exit code" 2 o.Cli.code;
  Alcotest.(check bool) "usage on stderr" true (contains o.Cli.err "usage")

let run_compile_dir_read_failure () =
  (* A file that lists but fails to read aborts with the io error. *)
  let list_files _ = [ "a.tono" ] in
  let read_file p = raise (Sys_error (p ^ ": permission denied")) in
  let o = Cli.run ~list_files ~read_file [| "x"; "compile-dir"; "/proj" |] in
  Alcotest.(check int) "io error exit code" 1 o.Cli.code;
  Alcotest.(check bool)
    "reports the io error" true
    (contains o.Cli.err "permission denied")

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
          Alcotest.test_case "fmt" `Quick run_fmt;
          Alcotest.test_case "fmt invalid source" `Quick run_fmt_invalid_source;
          Alcotest.test_case "fmt missing path" `Quick run_fmt_missing_path;
          Alcotest.test_case "fmt file not found" `Quick run_fmt_file_not_found;
          Alcotest.test_case "compile-dir happy path" `Quick run_compile_dir_ok;
          Alcotest.test_case "compile-dir visibility" `Quick
            run_compile_dir_visibility;
          Alcotest.test_case "compile-dir cycle" `Quick run_compile_dir_cycle;
          Alcotest.test_case "compile-dir empty" `Quick run_compile_dir_empty;
          Alcotest.test_case "compile-dir missing root" `Quick
            run_compile_dir_missing_root;
          Alcotest.test_case "compile-dir missing arg" `Quick
            run_compile_dir_missing_arg;
          Alcotest.test_case "compile-dir read failure" `Quick
            run_compile_dir_read_failure;
        ] );
    ]
