(* Protocol-level regressions against the real server binary over a pipe. The
   pure [Analysis] suite cannot catch these: they live in the transport loop
   (malformed frames, lifecycle notifications), so each test spawns the built
   executable, speaks raw framed JSON-RPC on stdin, and inspects everything
   the server wrote before exiting. *)

let exe = "../tono_lsp.exe"

let frame body =
  Printf.sprintf "Content-Length: %d\r\n\r\n%s" (String.length body) body

let parse_frames (s : string) : Yojson.Safe.t list =
  let find_sub needle from =
    let n = String.length s and m = String.length needle in
    let rec go i =
      if i + m > n then None
      else if String.sub s i m = needle then Some i
      else go (i + 1)
    in
    go from
  in
  let rec collect i acc =
    match find_sub "Content-Length:" i with
    | None -> List.rev acc
    | Some h -> (
        match find_sub "\r\n\r\n" h with
        | None -> List.rev acc
        | Some sep ->
            let header = String.sub s (h + 15) (sep - h - 15) in
            let len =
              int_of_string
                (String.trim
                   (List.hd (String.split_on_char '\r' (String.trim header))))
            in
            let body = String.sub s (sep + 4) len in
            collect (sep + 4 + len) (Yojson.Safe.from_string body :: acc))
  in
  collect 0 []

(* Send [bodies] (the last one must be the exit notification so the server
   terminates on its own), then return every JSON payload the server wrote
   plus its exit status. *)
let session (bodies : string list) : Yojson.Safe.t list * Unix.process_status =
  let ic, oc = Unix.open_process exe in
  List.iter (fun b -> output_string oc (frame b)) bodies;
  flush oc;
  let buf = Buffer.create 4096 in
  (try
     while true do
       Buffer.add_channel buf ic 1
     done
   with End_of_file -> ());
  let status = Unix.close_process (ic, oc) in
  (parse_frames (Buffer.contents buf), status)

let member name json =
  match json with `Assoc fields -> List.assoc_opt name fields | _ -> None

let response_for (id : int) (frames : Yojson.Safe.t list) : Yojson.Safe.t option
    =
  List.find_opt (fun f -> member "id" f = Some (`Int id)) frames

let exited_cleanly = function Unix.WEXITED 0 -> true | _ -> false

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
    ]
