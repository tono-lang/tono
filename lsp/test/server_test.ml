(* Protocol-level regressions against the real server binary over a pipe. The
   pure [Analysis] suite cannot catch these: they live in the transport loop
   (malformed frames, lifecycle notifications), so each test spawns the built
   executable, speaks raw framed JSON-RPC on stdin, and inspects everything
   the server wrote before exiting. *)

open Server_harness

(* A `params: null` request (forbidden by JSON-RPC) and a request whose body
   has the wrong shape must both be answered as errors, with the server alive
   for the valid requests that follow. *)
let malformed_messages_get_error_responses () =
  let frames, status =
    session
      [
        {|{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}|};
        {|{"jsonrpc":"2.0","id":2,"method":"shutdown","params":null}|};
        {|{"jsonrpc":"2.0","id":3,"method":"textDocument/hover","params":{"textDocument":5}}|};
        {|{"jsonrpc":"2.0","id":4,"method":"shutdown"}|};
        {|{"jsonrpc":"2.0","method":"exit"}|};
      ]
  in
  Alcotest.(check bool) "server exits cleanly" true (exited_cleanly status);
  let has_error id =
    match response_for id frames with
    | Some r -> member "error" r <> None
    | None -> false
  in
  Alcotest.(check bool)
    "initialize succeeds" true
    (match response_for 1 frames with
    | Some r -> member "result" r <> None
    | None -> false);
  Alcotest.(check bool) "null params answered as an error" true (has_error 2);
  Alcotest.(check bool)
    "wrong-shape body answered as an error" true (has_error 3);
  Alcotest.(check bool)
    "server still answers after the bad ones" true
    (response_for 4 frames <> None)

(* Closing a document must clear its published diagnostics, or the editor's
   problem list keeps entries for a buffer that no longer exists. *)
let did_close_clears_diagnostics () =
  let frames, status =
    session
      [
        {|{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}|};
        {|{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.tono","languageId":"tono","version":1,"text":"struct box { it: missing }"}}}|};
        {|{"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":"file:///t.tono"}}}|};
        shutdown_body;
        {|{"jsonrpc":"2.0","method":"exit"}|};
      ]
  in
  Alcotest.(check bool) "server exits cleanly" true (exited_cleanly status);
  let published =
    List.filter_map
      (fun f ->
        if member "method" f = Some (`String "textDocument/publishDiagnostics")
        then Option.bind (member "params" f) (member "diagnostics")
        else None)
      frames
  in
  match published with
  | [ `List first; `List last ] ->
      Alcotest.(check bool) "open publishes the error" true (first <> []);
      Alcotest.(check int) "close clears diagnostics" 0 (List.length last)
  | _ -> Alcotest.fail "expected exactly two publishDiagnostics notifications"

(* Positions must speak UTF-16 through the real server too: with multi-byte
   content before the hovered symbol ("ação" is 6 bytes, 4 code units), the
   answered range counts code units, not bytes. *)
let hover_range_counts_utf16_units () =
  let frames, status =
    session
      [
        {|{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}|};
        {|{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///t.tono","languageId":"tono","version":1,"text":"@doc(\"ação\") struct point { at: edge }\nstruct edge { x: i64 }"}}}|};
        {|{"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///t.tono"},"position":{"line":0,"character":20}}}|};
        shutdown_body;
        {|{"jsonrpc":"2.0","method":"exit"}|};
      ]
  in
  Alcotest.(check bool) "server exits cleanly" true (exited_cleanly status);
  match response_for 2 frames with
  | None -> Alcotest.fail "expected a hover response"
  | Some r -> (
      let character =
        Option.bind (member "result" r) (fun result ->
            Option.bind (member "range" result) (fun range ->
                Option.bind (member "start" range) (member "character")))
      in
      match character with
      | Some (`Int c) ->
          Alcotest.(check int) "range starts at the utf16 column" 20 c
      | _ -> Alcotest.fail "expected a hover range")

(* --- workspace protocol tests --- *)

(* A throwaway two-module project (src convention, no manifest): pay.common
   declares [money], pay.charges references it through an import. *)
let make_project () : string * string * string =
  let base = Filename.temp_file "tono-lsp-proj" "" in
  Sys.remove base;
  Unix.mkdir base 0o755;
  let src = Filename.concat base "src" in
  Unix.mkdir src 0o755;
  let pay = Filename.concat src "pay" in
  Unix.mkdir pay 0o755;
  let common = Filename.concat pay "common.tono" in
  write_file common "pub struct money { amount: i64 }\n";
  let charges = Filename.concat pay "charges.tono" in
  write_file charges
    "import pay.common\n\nstruct charge { total: common.money }\n";
  (base, common, charges)

let uri_of path = "file://" ^ path
let json_string (s : string) : string = Yojson.Safe.to_string (`String s)

let didopen_body path =
  Printf.sprintf
    {|{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":%s,"languageId":"tono","version":1,"text":%s}}}|}
    (json_string (uri_of path))
    (json_string (read_file path))

(* The (line, character) of the name behind "common." in [charges]. *)
let ref_position charges =
  let text = read_file charges in
  let lines = String.split_on_char '\n' text in
  let line, l =
    let rec find i = function
      | [] -> Alcotest.fail "no qualified reference in the fixture"
      | l :: rest ->
          let has =
            let rec go j =
              j + 7 <= String.length l
              && (String.sub l j 7 = "common." || go (j + 1))
            in
            go 0
          in
          if has then (i, l) else find (i + 1) rest
    in
    find 0 lines
  in
  let col =
    let rec go j = if String.sub l j 7 = "common." then j else go (j + 1) in
    go 0
  in
  (line, col + 8)

(* The acceptance case: a valid multi-module file opens with zero
   diagnostics, because imports resolve with project context. *)
let project_open_is_clean () =
  let _base, _common, charges = make_project () in
  let frames, status =
    session [ init_body; didopen_body charges; shutdown_body; exit_body ]
  in
  Alcotest.(check bool) "server exits cleanly" true (exited_cleanly status);
  let published =
    List.filter_map
      (fun f ->
        if member "method" f = Some (`String "textDocument/publishDiagnostics")
        then Option.bind (member "params" f) (member "diagnostics")
        else None)
      frames
  in
  match published with
  | [ `List ds ] ->
      Alcotest.(check int) "no false diagnostics" 0 (List.length ds)
  | _ -> Alcotest.fail "expected exactly one publishDiagnostics"

let cross_file_definition () =
  let _base, common, charges = make_project () in
  let line, col = ref_position charges in
  let body =
    Printf.sprintf
      {|{"jsonrpc":"2.0","id":2,"method":"textDocument/definition","params":{"textDocument":{"uri":%s},"position":{"line":%d,"character":%d}}}|}
      (json_string (uri_of charges))
      line col
  in
  let frames, _ =
    session [ init_body; didopen_body charges; body; shutdown_body; exit_body ]
  in
  match response_for 2 frames with
  | None -> Alcotest.fail "expected a definition response"
  | Some r -> (
      match member "result" r with
      | Some (`List [ loc ]) -> (
          match member "uri" loc with
          | Some (`String uri) ->
              Alcotest.(check bool)
                "lands in the other file" true
                (uri = uri_of common)
          | _ -> Alcotest.fail "expected a location uri")
      | _ -> Alcotest.fail "expected one location")

let workspace_rename_and_collision () =
  let _base, common, charges = make_project () in
  (* A second declaration in the target module makes 'cash' a collision. *)
  write_file common
    "pub struct money { amount: i64 }\n\npub struct cash { amount: i64 }\n";
  let line, col = ref_position charges in
  let rename id new_name =
    Printf.sprintf
      {|{"jsonrpc":"2.0","id":%d,"method":"textDocument/rename","params":{"textDocument":{"uri":%s},"position":{"line":%d,"character":%d},"newName":%s}}|}
      id
      (json_string (uri_of charges))
      line col (json_string new_name)
  in
  let frames, _ =
    session
      [
        init_body;
        didopen_body charges;
        rename 2 "funds";
        rename 3 "cash";
        exit_body;
      ]
  in
  (match response_for 2 frames with
  | None -> Alcotest.fail "expected a rename response"
  | Some r -> (
      match Option.bind (member "result" r) (member "changes") with
      | Some (`Assoc changes) ->
          Alcotest.(check int) "edits touch both files" 2 (List.length changes);
          Alcotest.(check bool)
            "declaration file edited" true
            (List.mem_assoc (uri_of common) changes)
      | _ -> Alcotest.fail "expected a workspace edit"));
  match response_for 3 frames with
  | Some r ->
      Alcotest.(check bool)
        "colliding rename is refused" true
        (member "error" r <> None)
  | None -> Alcotest.fail "expected a response to the colliding rename"

let add_import_code_action () =
  let base, _common, _charges = make_project () in
  (* A third module referencing pay.common without importing it. *)
  let broken = Filename.concat (Filename.concat base "src") "billing.tono" in
  write_file broken "struct invoice { total: common.money }\n";
  let action_req =
    Printf.sprintf
      {|{"jsonrpc":"2.0","id":2,"method":"textDocument/codeAction","params":{"textDocument":{"uri":%s},"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":39}},"context":{"diagnostics":[]}}}|}
      (json_string (uri_of broken))
  in
  let frames, _ =
    session
      [ init_body; didopen_body broken; action_req; shutdown_body; exit_body ]
  in
  match response_for 2 frames with
  | None -> Alcotest.fail "expected a code action response"
  | Some r -> (
      match member "result" r with
      | Some (`List (action :: _)) ->
          Alcotest.(check bool)
            "offers the import" true
            (member "title" action = Some (`String "import pay.common"));
          let text = Yojson.Safe.to_string action in
          Alcotest.(check bool)
            "the edit inserts the canonical import" true
            (let needle = {|"newText":"import pay.common|} in
             let hay = text in
             let lh = String.length hay and ln = String.length needle in
             let rec go i =
               i + ln <= lh && (String.sub hay i ln = needle || go (i + 1))
             in
             go 0)
      | _ -> Alcotest.fail "expected at least one quick fix")

let watched_file_change_republishes () =
  let _base, common, charges = make_project () in
  let ic, oc = Unix.open_process exe in
  let send body =
    output_string oc (frame body);
    flush oc
  in
  let read_one () : Yojson.Safe.t option =
    let rec len acc =
      match input_line ic with
      | exception End_of_file -> None
      | "" | "\r" -> acc
      | line -> (
          let line = String.trim line in
          match String.index_opt line ':' with
          | Some i
            when String.lowercase_ascii (String.sub line 0 i) = "content-length"
            ->
              len
                (int_of_string_opt
                   (String.trim
                      (String.sub line (i + 1) (String.length line - i - 1))))
          | _ -> len acc)
    in
    match len None with
    | None -> None
    | Some n ->
        let b = Bytes.create n in
        really_input ic b 0 n;
        Some (Yojson.Safe.from_string (Bytes.to_string b))
  in
  let rec wait_publish () =
    match read_one () with
    | None -> Alcotest.fail "eof before diagnostics arrived"
    | Some f
      when member "method" f = Some (`String "textDocument/publishDiagnostics")
      ->
        Option.bind (member "params" f) (member "diagnostics")
    | Some _ -> wait_publish ()
  in
  send init_body;
  send (didopen_body charges);
  (match wait_publish () with
  | Some (`List []) -> ()
  | _ -> Alcotest.fail "the project opens clean");
  (* The imported file changes on disk: [money] disappears, so the open
     dependent must re-publish with a real error. *)
  write_file common "pub struct cash { amount: i64 }\n";
  send
    (Printf.sprintf
       {|{"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[{"uri":%s,"type":2}]}}|}
       (json_string (uri_of common)));
  (match wait_publish () with
  | Some (`List (_ :: _)) -> ()
  | _ -> Alcotest.fail "the dependent re-publishes a real diagnostic");
  send shutdown_body;
  send exit_body;
  (try
     while true do
       ignore (input_char ic)
     done
   with End_of_file -> ());
  match Unix.close_process (ic, oc) with
  | Unix.WEXITED 0 -> ()
  | _ -> Alcotest.fail "server exits cleanly"

let workspace_symbol_search () =
  let _base, common, charges = make_project () in
  let body =
    {|{"jsonrpc":"2.0","id":2,"method":"workspace/symbol","params":{"query":"money"}}|}
  in
  let frames, _ =
    session [ init_body; didopen_body charges; body; shutdown_body; exit_body ]
  in
  match response_for 2 frames with
  | None -> Alcotest.fail "expected a workspace/symbol response"
  | Some r -> (
      match member "result" r with
      | Some (`List (sym :: _)) ->
          Alcotest.(check bool)
            "found in the declaring file" true
            ( Yojson.Safe.to_string sym |> fun s ->
              let needle = uri_of common in
              let lh = String.length s and ln = String.length needle in
              let rec go i =
                i + ln <= lh && (String.sub s i ln = needle || go (i + 1))
              in
              go 0 )
      | _ -> Alcotest.fail "expected at least one symbol")

let signature_help_in_trait () =
  let src =
    "op f(a): a\n  @http(method: \"GET\", path: \"/x\")\nstruct a { x: i64 }"
  in
  let opened =
    Printf.sprintf
      {|{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///sig.tono","languageId":"tono","version":1,"text":%s}}}|}
      (json_string
         "@http(method: \"GET\", path: \"/x\")\nop f(a): a\nstruct a { x: i64 }")
  in
  ignore src;
  let body =
    {|{"jsonrpc":"2.0","id":2,"method":"textDocument/signatureHelp","params":{"textDocument":{"uri":"file:///sig.tono"},"position":{"line":0,"character":7}}}|}
  in
  let frames, _ =
    session [ init_body; opened; body; shutdown_body; exit_body ]
  in
  match response_for 2 frames with
  | None -> Alcotest.fail "expected a signature help response"
  | Some r ->
      let s = Yojson.Safe.to_string r in
      Alcotest.(check bool)
        "lists the trait keys" true
        (let needle = "path: string" in
         let lh = String.length s and ln = String.length needle in
         let rec go i =
           i + ln <= lh && (String.sub s i ln = needle || go (i + 1))
         in
         go 0)

(* Differential regression: an unsaved buffer must be exactly as strict as the
   compiler. A file with a workspace-resolvable qualifier but no import gets
   TC0023 (not silence), the add-import quick fix is offered, and applying its
   edit clears the diagnostic. *)
let unsaved_buffer_matches_the_compiler () =
  let base, _common, _charges = make_project () in
  (* The buffer's path sits under the project but does not exist on disk. *)
  let path = Filename.concat (Filename.concat base "src") "billing.tono" in
  let src = "struct invoice { total: common.money }\n" in
  let fixed = "import pay.common\n\n" ^ src in
  let opened =
    Printf.sprintf
      {|{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":%s,"languageId":"tono","version":1,"text":%s}}}|}
      (json_string (uri_of path))
      (json_string src)
  in
  let action_req =
    Printf.sprintf
      {|{"jsonrpc":"2.0","id":2,"method":"textDocument/codeAction","params":{"textDocument":{"uri":%s},"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":39}},"context":{"diagnostics":[]}}}|}
      (json_string (uri_of path))
  in
  let changed =
    Printf.sprintf
      {|{"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":%s,"version":2},"contentChanges":[{"text":%s}]}}|}
      (json_string (uri_of path))
      (json_string fixed)
  in
  let frames, status =
    session [ init_body; opened; action_req; changed; shutdown_body; exit_body ]
  in
  Alcotest.(check bool) "server exits cleanly" true (exited_cleanly status);
  let published =
    List.filter_map
      (fun f ->
        if member "method" f = Some (`String "textDocument/publishDiagnostics")
        then Option.bind (member "params" f) (member "diagnostics")
        else None)
      frames
  in
  (match published with
  | [ `List first; `List second ] ->
      Alcotest.(check bool)
        "the missing import is diagnosed, like the compiler" true
        (List.exists (fun d -> member "code" d = Some (`String "TC0023")) first);
      Alcotest.(check int) "the applied edit clears it" 0 (List.length second)
  | _ -> Alcotest.fail "expected two publishDiagnostics");
  match response_for 2 frames with
  | None -> Alcotest.fail "expected a code action response"
  | Some r -> (
      match member "result" r with
      | Some (`List (action :: _)) ->
          Alcotest.(check bool)
            "offers the import fix" true
            (member "title" action = Some (`String "import pay.common"))
      | _ -> Alcotest.fail "expected the add-import quick fix")

(* A rename target must be a name the parser accepts: keywords, primitive
   names, malformed identifiers, and the empty string are refused before any
   edit is built (applying them would corrupt the file). *)
let rename_validates_the_new_name () =
  let _base, _common, charges = make_project () in
  let line, col = ref_position charges in
  let rename id new_name =
    Printf.sprintf
      {|{"jsonrpc":"2.0","id":%d,"method":"textDocument/rename","params":{"textDocument":{"uri":%s},"position":{"line":%d,"character":%d},"newName":%s}}|}
      id
      (json_string (uri_of charges))
      line col (json_string new_name)
  in
  let bad = [ "struct"; "i64"; "Bad-Name"; "has space"; "" ] in
  let frames, _ =
    session
      ([ init_body; didopen_body charges ]
      @ List.mapi (fun i n -> rename (i + 2) n) bad
      @ [ rename 7 "funds"; shutdown_body; exit_body ])
  in
  List.iteri
    (fun i n ->
      match response_for (i + 2) frames with
      | Some r ->
          Alcotest.(check bool)
            (Printf.sprintf "rename to %S is refused" n)
            true
            (member "error" r <> None)
      | None -> Alcotest.fail "expected a response to the invalid rename")
    bad;
  match response_for 7 frames with
  | Some r ->
      Alcotest.(check bool)
        "a valid free name still renames" true
        (Option.bind (member "result" r) (member "changes") <> None)
  | None -> Alcotest.fail "expected a response to the valid rename"

(* An import cycle is every participant's error: the open file must show
   TC0025 on its own import line, and breaking the cycle must clear it. *)
let import_cycle_is_diagnosed_and_clears () =
  let base = Filename.temp_file "tono-lsp-cycle" "" in
  Sys.remove base;
  Unix.mkdir base 0o755;
  let src = Filename.concat base "src" in
  Unix.mkdir src 0o755;
  let a = Filename.concat src "a" in
  Unix.mkdir a 0o755;
  let one = Filename.concat a "one.tono" in
  write_file one "import a.two\n\npub struct one_t { peer: two.two_t }\n";
  let two = Filename.concat a "two.tono" in
  write_file two "import a.one\n\npub struct two_t { peer: one.one_t }\n";
  let changed =
    Printf.sprintf
      {|{"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":%s,"version":2},"contentChanges":[{"text":%s}]}}|}
      (json_string (uri_of one))
      (json_string "pub struct one_t { x: i64 }\n")
  in
  let frames, status =
    session [ init_body; didopen_body one; changed; shutdown_body; exit_body ]
  in
  Alcotest.(check bool) "server exits cleanly" true (exited_cleanly status);
  let published =
    List.filter_map
      (fun f ->
        if member "method" f = Some (`String "textDocument/publishDiagnostics")
        then Option.bind (member "params" f) (member "diagnostics")
        else None)
      frames
  in
  match published with
  | [ `List first; `List second ] ->
      Alcotest.(check bool)
        "the cycle is diagnosed in the open file" true
        (List.exists (fun d -> member "code" d = Some (`String "TC0025")) first);
      Alcotest.(check int) "breaking the cycle clears it" 0 (List.length second)
  | _ -> Alcotest.fail "expected two publishDiagnostics"

(* The lifecycle gate (spec #initialize/#shutdown/#exit): requests before
   initialize answer ServerNotInitialized, initialize is once-only, requests
   after shutdown are invalid, and exit without shutdown reports code 1. *)
let lifecycle_gates_dispatch () =
  let hover id =
    Printf.sprintf
      {|{"jsonrpc":"2.0","id":%d,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///t.tono"},"position":{"line":0,"character":0}}}|}
      id
  in
  let init id =
    Printf.sprintf
      {|{"jsonrpc":"2.0","id":%d,"method":"initialize","params":{"capabilities":{}}}|}
      id
  in
  let frames, status =
    session [ hover 2; init 3; init 4; shutdown_body; hover 5; exit_body ]
  in
  Alcotest.(check bool) "clean exit after shutdown" true (exited_cleanly status);
  let error_code id =
    Option.bind (response_for id frames) (fun r ->
        Option.bind (member "error" r) (member "code"))
  in
  Alcotest.(check bool)
    "pre-initialize request is ServerNotInitialized" true
    (error_code 2 = Some (`Int (-32002)));
  Alcotest.(check bool)
    "initialize succeeds once" true
    (Option.bind (response_for 3 frames) (member "result") <> None);
  Alcotest.(check bool)
    "second initialize is an error" true
    (error_code 4 <> None);
  Alcotest.(check bool)
    "request after shutdown is an error" true
    (error_code 5 = Some (`Int (-32600)))

let exit_without_shutdown_is_abnormal () =
  let _frames, status =
    session [ init_body; {|{"jsonrpc":"2.0","method":"exit"}|} ]
  in
  match status with
  | Unix.WEXITED 1 -> ()
  | _ -> Alcotest.fail "exit without shutdown must report code 1"

let () =
  Alcotest.run "server"
    [
      ( "protocol",
        [
          Alcotest.test_case "malformed messages" `Quick
            malformed_messages_get_error_responses;
          Alcotest.test_case "didClose clears diagnostics" `Quick
            did_close_clears_diagnostics;
          Alcotest.test_case "utf16 hover range" `Quick
            hover_range_counts_utf16_units;
        ] );
      ( "workspace",
        [
          Alcotest.test_case "project opens clean" `Quick project_open_is_clean;
          Alcotest.test_case "cross-file definition" `Quick
            cross_file_definition;
          Alcotest.test_case "rename and collision" `Quick
            workspace_rename_and_collision;
          Alcotest.test_case "add-import code action" `Quick
            add_import_code_action;
          Alcotest.test_case "watched file change" `Quick
            watched_file_change_republishes;
          Alcotest.test_case "workspace symbol" `Quick workspace_symbol_search;
          Alcotest.test_case "signature help" `Quick signature_help_in_trait;
          Alcotest.test_case "unsaved buffer strictness" `Quick
            unsaved_buffer_matches_the_compiler;
          Alcotest.test_case "rename name validation" `Quick
            rename_validates_the_new_name;
          Alcotest.test_case "import cycle" `Quick
            import_cycle_is_diagnosed_and_clears;
          Alcotest.test_case "lifecycle gate" `Quick lifecycle_gates_dispatch;
          Alcotest.test_case "abnormal exit code" `Quick
            exit_without_shutdown_is_abnormal;
        ] );
    ]
