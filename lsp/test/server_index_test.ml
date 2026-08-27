(* The library index over the protocol, against a stand-in `tono`: the
   first open builds it, a current index answers completion inside #(...)
   and the language-word hover without any process, and a stale one is
   discarded and rebuilt. *)

open Server_harness

let starts_with prefix s =
  String.length s >= String.length prefix
  && String.sub s 0 (String.length prefix) = prefix

let contains s sub =
  let ls = String.length s and lsub = String.length sub in
  let rec go i = i + lsub <= ls && (String.sub s i lsub = sub || go (i + 1)) in
  lsub = 0 || go 0

let source =
  {|ext gearbox {
  go { #(example.test/gearbox) }

  struct dial {
    go { #(Dial[float64]) }

    op read(): float {
      go { call: #(Read)() }
    }
  }

  op open(value: float): dial {
    go { call: #(Open[float64])(value) }
  }
}
|}

let manifest =
  "[project]\n\
   name = \"svc\"\n\n\
   [target.go]\n\
   out = \"sdk/go\"\n\n\
   [ext.gearbox]\n\
   go = \"v0.0.0\"\n"

let index_text ~version =
  Printf.sprintf
    {|{"tono_index_version":1,"key":{"ext":"gearbox","lang":"go","package":"example.test/gearbox","version":"%s","lockfile":{"path":"","digest":"none"},"format":1},"note":"Go export data carries no documentation","symbols":[{"name":"Dial","kind":"struct","signatures":["Dial[T any]"],"doc":"","members":[{"name":"Read","kind":"method","static":false,"signatures":["func() (T, error)"]}]},{"name":"Open","kind":"function","signatures":["func[T any](value T) (Dial[T], error)"],"doc":"","members":[]}]}|}
    version

(* A stand-in `tono` whose `index` writes the index file the real one
   would, keyed to the project, and prints the built line. *)
let fake_tono_script =
  {|#!/bin/sh
printf '%s\n' "$*" >> "$TONO_LSP_TEST_LOG"
case "$1" in
  index)
    mkdir -p "$TONO_LSP_TEST_DIR/.tono/index"
    cp "$TONO_LSP_TEST_INDEX" "$TONO_LSP_TEST_DIR/.tono/index/gearbox.go.json"
    printf '%s\n' '{"kind":"built","ext":"gearbox","lang":"go","package":"example.test/gearbox","version":"v0.0.0","path":"x","symbols":2}';;
  *) printf '%s\n' '{"kind":"unchecked","message":"no check in this stand-in"}';;
esac
|}

(* A project directory with the manifest and the source, the fake on
   TONO_BIN, and the index the fake writes on demand. *)
let with_project (f : dir:string -> log:string -> unit) : unit =
  let dir = Filename.temp_file "tono-lsp-index" "" in
  Sys.remove dir;
  Unix.mkdir dir 0o755;
  let script = Filename.concat dir "tono" in
  write_file script fake_tono_script;
  Unix.chmod script 0o755;
  let log = Filename.concat dir "calls.log" in
  write_file log "";
  let built = Filename.concat dir "built.json" in
  write_file built (index_text ~version:"v0.0.0");
  write_file (Filename.concat dir "tono.toml") manifest;
  write_file (Filename.concat dir "svc.tono") source;
  Unix.putenv "TONO_BIN" script;
  Unix.putenv "TONO_LSP_TEST_LOG" log;
  Unix.putenv "TONO_LSP_TEST_DIR" dir;
  Unix.putenv "TONO_LSP_TEST_INDEX" built;
  let rec rm path =
    if Sys.is_directory path then begin
      Array.iter (fun n -> rm (Filename.concat path n)) (Sys.readdir path);
      Unix.rmdir path
    end
    else Sys.remove path
  in
  Fun.protect
    ~finally:(fun () ->
      Unix.putenv "TONO_BIN" "";
      try rm dir with _ -> ())
    (fun () -> f ~dir ~log)

let uri_of dir = "file://" ^ Filename.concat dir "svc.tono"

let did_open uri =
  Printf.sprintf
    {|{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"%s","languageId":"tono","version":1,"text":%s}}}|}
    uri
    (Yojson.Safe.to_string (`String source))

let completion ~id uri ~line ~character =
  Printf.sprintf
    {|{"jsonrpc":"2.0","id":%d,"method":"textDocument/completion","params":{"textDocument":{"uri":"%s"},"position":{"line":%d,"character":%d}}}|}
    id uri line character

let hover ~id uri ~line ~character =
  Printf.sprintf
    {|{"jsonrpc":"2.0","id":%d,"method":"textDocument/hover","params":{"textDocument":{"uri":"%s"},"position":{"line":%d,"character":%d}}}|}
    id uri line character

let labels_of (frames : Yojson.Safe.t list) (id : int) : string list =
  match Option.bind (response_for id frames) (member "result") with
  | Some (`List items) ->
      List.filter_map
        (fun i ->
          match member "label" i with Some (`String l) -> Some l | _ -> None)
        items
  | _ -> []

let hover_of (frames : Yojson.Safe.t list) (id : int) : string =
  match Option.bind (response_for id frames) (member "result") with
  | Some r -> (
      match Option.bind (member "contents" r) (member "value") with
      | Some (`String v) -> v
      | _ -> "")
  | None -> ""

let index_logs frames =
  List.filter_map
    (fun f ->
      if member "method" f = Some (`String "window/logMessage") then
        match Option.bind (member "params" f) (member "message") with
        | Some (`String m) when starts_with "index " m -> Some m
        | _ -> None
      else None)
    frames

(* The call: head of op open and the storage of struct dial, and the
   language word of the storage block. *)
let call_head = (12, String.length "    go { call: #(")
let storage = (4, String.length "    go { #(")
let lang_word = (4, 5)

let the_first_open_builds_the_index () =
  with_project (fun ~dir ~log ->
      let uri = uri_of dir in
      let frames, status =
        session [ init_body; did_open uri; shutdown_body; exit_body ]
      in
      Alcotest.(check bool) "exit" true (exited_cleanly status);
      let calls = read_file log in
      Alcotest.(check bool)
        "index called for the go pair" true
        (contains calls
           (Printf.sprintf "index %s --json --only gearbox=go"
              (Filename.concat dir "svc.tono")));
      Alcotest.(check bool)
        "index written" true
        (Sys.file_exists (Filename.concat dir ".tono/index/gearbox.go.json"));
      match index_logs frames with
      | [ m ] ->
          Alcotest.(check bool)
            "log line" true
            (starts_with "index svc.tono: 1 of 1 pairs built (gearbox/go)" m)
      | other ->
          Alcotest.fail
            (Printf.sprintf "expected one index log, got %d" (List.length other)))

let a_current_index_answers_without_a_process () =
  with_project (fun ~dir ~log ->
      let uri = uri_of dir in
      write_file
        ( Filename.concat dir ".tono/index/gearbox.go.json" |> fun p ->
          (try Unix.mkdir (Filename.concat dir ".tono") 0o755 with _ -> ());
          (try Unix.mkdir (Filename.concat dir ".tono/index") 0o755
           with _ -> ());
          p )
        (index_text ~version:"v0.0.0");
      let frames, status =
        session
          [
            init_body;
            did_open uri;
            completion ~id:2 uri ~line:(fst call_head)
              ~character:(snd call_head);
            completion ~id:3 uri ~line:(fst storage) ~character:(snd storage);
            hover ~id:4 uri ~line:(fst lang_word) ~character:(snd lang_word);
            shutdown_body;
            exit_body;
          ]
      in
      Alcotest.(check bool) "exit" true (exited_cleanly status);
      Alcotest.(check (list string))
        "call head" [ "Dial"; "Open" ] (labels_of frames 2);
      Alcotest.(check (list string)) "storage" [ "Dial" ] (labels_of frames 3);
      let h = hover_of frames 4 in
      Alcotest.(check bool)
        "hover counts the symbols" true
        (contains h "2 symbols of example.test/gearbox v0.0.0");
      Alcotest.(check bool)
        "hover carries the note" true
        (contains h "carries no documentation");
      Alcotest.(check string) "no build ran" "" (read_file log))

let a_stale_index_is_discarded_and_rebuilt () =
  with_project (fun ~dir ~log ->
      let uri = uri_of dir in
      (try Unix.mkdir (Filename.concat dir ".tono") 0o755 with _ -> ());
      (try Unix.mkdir (Filename.concat dir ".tono/index") 0o755 with _ -> ());
      let index = Filename.concat dir ".tono/index/gearbox.go.json" in
      write_file index (index_text ~version:"v9.9.9");
      let frames, status =
        session
          [
            init_body;
            completion ~id:2 uri ~line:(fst call_head)
              ~character:(snd call_head);
            did_open uri;
            hover ~id:4 uri ~line:(fst lang_word) ~character:(snd lang_word);
            shutdown_body;
            exit_body;
          ]
      in
      Alcotest.(check bool) "exit" true (exited_cleanly status);
      Alcotest.(check (list string))
        "unknown document offers nothing" [] (labels_of frames 2);
      Alcotest.(check bool)
        "hover says stale" true
        (contains (hover_of frames 4) "stale");
      Alcotest.(check bool)
        "rebuilt" true
        (contains (read_file log) "--only gearbox=go");
      (* The fake wrote the current index over the stale one. *)
      Alcotest.(check bool)
        "current on disk" true
        (contains (read_file index) "\"version\":\"v0.0.0\""))

let () =
  Alcotest.run "server_index"
    [
      ( "index",
        [
          Alcotest.test_case "first open builds" `Quick
            the_first_open_builds_the_index;
          Alcotest.test_case "current index answers" `Quick
            a_current_index_answers_without_a_process;
          Alcotest.test_case "stale index rebuilt" `Quick
            a_stale_index_is_discarded_and_rebuilt;
        ] );
    ]
