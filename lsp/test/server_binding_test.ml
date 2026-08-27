(* The foreign-binding check on save, against the real server binary: a
   stand-in `tono` answers the check's JSON report, so the suite pins how
   the server schedules pairs, merges verdicts, and publishes them, with
   nothing else in the loop. *)

open Server_harness

(* An ext bound in three languages; every edit below keeps it frontend-clean
   so the save runs the check. *)
let bound_source =
  {|ext gearbox {
  go { #(example.test/gearbox) }
  ts { #(@example/gearbox) }
  rust { #(gearbox) }

  struct dial {
    go { #(Dial[float64]) }
    ts { #(Dial<number>) }
    rust { #(Dial<f64>) }

    op read(): float {
      go { call: #(Read)(#(ctx context.Context)) }
      ts { call: #(read)() }
      rust { call: #(read)() }
    }
  }

  op open(value: float): dial {
    go { call: #(Open[float64])(value) }
    ts { call: #(new Dial)(value) }
    rust { call: #(Dial::open)(value) }
  }
}
|}

let replace ~a ~b s =
  let n = String.length s and m = String.length a in
  let rec find i =
    if i + m > n then None
    else if String.sub s i m = a then Some i
    else find (i + 1)
  in
  match find 0 with
  | Some i -> String.sub s 0 i ^ b ^ String.sub s (i + m) (n - i - m)
  | None -> failwith ("fixture has no " ^ a)

(* A stand-in `tono` that records each invocation and answers a canned
   report per pair: a finding for go, a check that could not run for ts,
   and the unchecked note for rust. The server never reads the report from
   anywhere else, so this is the whole contract it is tested against. *)
let fake_tono_script =
  {|#!/bin/sh
printf '%s\n' "$*" >> "$TONO_LSP_TEST_LOG"
case "$*" in
  *"gearbox=go"*) printf '%s\n' '{"kind":"finding","code":"FX0001","span":"19:16-43","message":"go binding of op open in ext gearbox: too many arguments","site":{"ext":"gearbox","lang":"go","kind":"op","owner":null,"name":"open","span":"19:16-43"}}'; exit 1;;
  *"gearbox=ts"*) printf '%s\n' '{"kind":"error","message":"checking the ts bindings of ext gearbox needs tsc, which is not installed"}'; exit 1;;
  *"gearbox=rust"*) printf '%s\n' '{"kind":"unchecked","message":"rust bindings of ext gearbox: nightly only"}';;
esac
|}

let with_fake_tono (f : log:string -> unit) : unit =
  let dir = Filename.temp_file "tono-lsp-fake" "" in
  Sys.remove dir;
  Unix.mkdir dir 0o755;
  let script = Filename.concat dir "tono" in
  write_file script fake_tono_script;
  Unix.chmod script 0o755;
  let log = Filename.concat dir "calls.log" in
  write_file log "";
  Unix.putenv "TONO_BIN" script;
  Unix.putenv "TONO_LSP_TEST_LOG" log;
  Fun.protect
    ~finally:(fun () ->
      Unix.putenv "TONO_BIN" "";
      List.iter
        (fun p -> try Sys.remove p with Sys_error _ -> ())
        [ script; log ];
      try Unix.rmdir dir with Unix.Unix_error _ -> ())
    (fun () -> f ~log)

let bound_uri = "file:///bound/svc.tono"

let did_open text =
  Printf.sprintf
    {|{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"%s","languageId":"tono","version":1,"text":%s}}}|}
    bound_uri
    (Yojson.Safe.to_string (`String text))

let did_change version text =
  Printf.sprintf
    {|{"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"%s","version":%d},"contentChanges":[{"text":%s}]}}|}
    bound_uri version
    (Yojson.Safe.to_string (`String text))

let did_save =
  Printf.sprintf
    {|{"jsonrpc":"2.0","method":"textDocument/didSave","params":{"textDocument":{"uri":"%s"}}}|}
    bound_uri

let published_for uri frames =
  List.filter_map
    (fun f ->
      if member "method" f = Some (`String "textDocument/publishDiagnostics")
      then
        match member "params" f with
        | Some p when member "uri" p = Some (`String uri) -> (
            match member "diagnostics" p with
            | Some (`List ds) -> Some ds
            | _ -> None)
        | _ -> None
      else None)
    frames

let binding_logs frames =
  List.filter_map
    (fun f ->
      if member "method" f = Some (`String "window/logMessage") then
        match Option.bind (member "params" f) (member "message") with
        | Some (`String m)
          when String.length m > 14 && String.sub m 0 14 = "binding check " ->
            Some m
        | _ -> None
      else None)
    frames

let starts_with prefix s =
  String.length s >= String.length prefix
  && String.sub s 0 (String.length prefix) = prefix

let last l = List.nth l (List.length l - 1)

(* Three saves: the first checks every pair, the second (one go block
   edited) re-checks the go pair alone, the third (an op signature edited)
   every pair again. The published verdict is the fake's report: the go
   finding re-located on the final text, the ts failure as a warning, the
   rust note as information, each where the check said. *)
let save_checks_dirty_pairs_and_publishes_the_verdict () =
  with_fake_tono (fun ~log ->
      let go_edited =
        replace ~a:"      go { call: #(Read)(#(ctx context.Context)) }"
          ~b:
            "      go {\n\
            \        // the same call, one line lower\n\
            \        call: #(Read)(#(ctx context.Context)) }"
          bound_source
      in
      let sig_edited =
        replace ~a:"open(value: float)" ~b:"open(value: i64)" go_edited
      in
      let frames, status =
        session
          [
            init_body;
            did_open bound_source;
            did_save;
            did_change 2 go_edited;
            did_save;
            did_change 3 sig_edited;
            did_save;
            shutdown_body;
            exit_body;
          ]
      in
      Alcotest.(check bool) "server exits cleanly" true (exited_cleanly status);
      let calls =
        List.filter
          (fun l -> l <> "")
          (String.split_on_char '\n' (read_file log))
      in
      Alcotest.(check int)
        "3 + 1 + 3 invocations of tono check" 7 (List.length calls);
      Alcotest.(check bool)
        "every call is a pair-scoped json check" true
        (List.for_all
           (fun c ->
             starts_with "check /bound/svc.tono --json --only gearbox=" c)
           calls);
      Alcotest.(check int)
        "the go pair ran three times" 3
        (List.length
           (List.filter (fun c -> String.ends_with ~suffix:"=go" c) calls));
      Alcotest.(check int)
        "the ts pair ran twice" 2
        (List.length
           (List.filter (fun c -> String.ends_with ~suffix:"=ts" c) calls));
      let logs = List.sort compare (binding_logs frames) in
      Alcotest.(check int) "one log line per save" 3 (List.length logs);
      Alcotest.(check bool)
        "the logs name what ran" true
        (List.exists
           (starts_with
              "binding check svc.tono: 1 of 3 pairs checked (gearbox/go)")
           logs
        && List.length
             (List.filter
                (starts_with "binding check svc.tono: 3 of 3 pairs checked")
                logs)
           = 2);
      let final = last (published_for bound_uri frames) in
      let of_kind sev =
        List.filter (fun d -> member "severity" d = Some (`Int sev)) final
      in
      (match of_kind 1 with
      | [ finding ] ->
          Alcotest.(check bool)
            "the finding carries the check's code" true
            (member "code" finding = Some (`String "FX0001"));
          let line =
            Option.bind (member "range" finding) (fun r ->
                Option.bind (member "start" r) (member "line"))
          in
          (* The fake reports line 19 (the text it was told about); the go
             edit pushed the binding two lines down, and the site puts it
             there. *)
          Alcotest.(check (option int))
            "re-located on the current text" (Some 20)
            (match line with Some (`Int l) -> Some l | _ -> None)
      | ds ->
          Alcotest.fail
            (Printf.sprintf "expected one FX0001 error, got %d" (List.length ds)));
      (match of_kind 2 with
      | [ warning ] ->
          Alcotest.(check bool)
            "the ts failure is a warning at its path line" true
            (member "message" warning
             = Some
                 (`String
                    "not checked: checking the ts bindings of ext gearbox \
                     needs tsc, which is not installed")
            && Option.bind (member "range" warning) (fun r ->
                   Option.bind (member "start" r) (member "line"))
               = Some (`Int 2))
      | ds ->
          Alcotest.fail
            (Printf.sprintf "expected one warning, got %d" (List.length ds)));
      match of_kind 3 with
      | [ note ] ->
          Alcotest.(check bool)
            "the rust note is information at its path line" true
            (member "message" note
             = Some
                 (`String
                    "not checked: rust bindings of ext gearbox: nightly only")
            && Option.bind (member "range" note) (fun r ->
                   Option.bind (member "start" r) (member "line"))
               = Some (`Int 3))
      | ds ->
          Alcotest.fail
            (Printf.sprintf "expected one note, got %d" (List.length ds)))

(* A file the frontend rejects is not checked (the check needs a source it
   accepts), and a `tono` that cannot run is reported on every pair, never
   swallowed. *)
let save_without_a_clean_source_or_a_binary () =
  with_fake_tono (fun ~log ->
      let broken =
        replace ~a:"open(value: float): dial {" ~b:"open(value: float): dial {{"
          bound_source
      in
      let frames, status =
        session
          [ init_body; did_open broken; did_save; shutdown_body; exit_body ]
      in
      Alcotest.(check bool) "server exits cleanly" true (exited_cleanly status);
      Alcotest.(check string)
        "no invocation for a rejected source" "" (read_file log);
      Alcotest.(check int)
        "no log line either" 0
        (List.length (binding_logs frames));
      Alcotest.(check bool)
        "only the frontend's error is published" true
        (List.for_all
           (fun d -> member "code" d <> Some (`String "FX0001"))
           (last (published_for bound_uri frames))));
  Unix.putenv "TONO_BIN" "/nonexistent/tono";
  let frames, status =
    session
      [ init_body; did_open bound_source; did_save; shutdown_body; exit_body ]
  in
  Unix.putenv "TONO_BIN" "";
  Alcotest.(check bool) "server exits cleanly" true (exited_cleanly status);
  let final = last (published_for bound_uri frames) in
  Alcotest.(check int) "one warning per pair" 3 (List.length final);
  Alcotest.(check bool)
    "each names the missing binary" true
    (List.for_all
       (fun d ->
         member "severity" d = Some (`Int 2)
         &&
         match member "message" d with
         | Some (`String m) ->
             starts_with "not checked: could not run /nonexistent/tono" m
         | _ -> false)
       final)

let () =
  Alcotest.run "server_binding"
    [
      ( "binding check",
        [
          Alcotest.test_case "save checks dirty pairs" `Quick
            save_checks_dirty_pairs_and_publishes_the_verdict;
          Alcotest.test_case "rejected source and missing binary" `Quick
            save_without_a_clean_source_or_a_binary;
        ] );
    ]
