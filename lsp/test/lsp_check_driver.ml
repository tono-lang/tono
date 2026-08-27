(* Drive the language server over a pipe the way an editor does (open the
   file, save it) and print what it published about the foreign bindings,
   rendered as `tono check` prints its report. The FFI bench diffs this
   against the command's own output: the editor and the command must agree
   byte for byte, or one of them is a second checker.

   With --latency N the file is saved N times, each save dirtying one
   language pair (a whitespace toggle inside the first language block), and
   the save-to-diagnostic time of each is measured from here, the client's
   side, with p50 and p95 printed at the end.

   usage: lsp_check_driver <tono_lsp.exe> <file.tono> [--latency N] *)

open Lsp.Types
module BC = Tono_lsp_lib.Binding_check

let frame body =
  Printf.sprintf "Content-Length: %d\r\n\r\n%s" (String.length body) body

let read_frame ic : Yojson.Safe.t option =
  let rec header len =
    match In_channel.input_line ic with
    | None -> None
    | Some line -> (
        let line = String.trim line in
        if line = "" then len
        else
          match String.index_opt line ':' with
          | Some i
            when String.lowercase_ascii (String.sub line 0 i) = "content-length"
            ->
              header
                (int_of_string_opt
                   (String.trim
                      (String.sub line (i + 1) (String.length line - i - 1))))
          | _ -> header len)
  in
  match header None with
  | None -> None
  | Some n -> (
      match really_input_string ic n with
      | body -> Some (Yojson.Safe.from_string body)
      | exception End_of_file -> None)

let member name json =
  match json with `Assoc fields -> List.assoc_opt name fields | _ -> None

let send oc body =
  output_string oc (frame body);
  flush oc

let json_string s = Yojson.Safe.to_string (`String s)

let binding_log json =
  if member "method" json = Some (`String "window/logMessage") then
    match Option.bind (member "params" json) (member "message") with
    | Some (`String m)
      when String.length m > 14 && String.sub m 0 14 = "binding check " ->
        Some m
    | _ -> None
  else None

let publish_for uri json =
  if member "method" json = Some (`String "textDocument/publishDiagnostics")
  then
    match member "params" json with
    | Some p when member "uri" p = Some (`String uri) -> (
        match member "diagnostics" p with
        | Some (`List ds) -> Some ds
        | _ -> None)
    | _ -> None
  else None

let ends_with ~suffix s =
  let n = String.length s and m = String.length suffix in
  n >= m && String.sub s (n - m) m = suffix

(* The server logs one line per save, then publishes the verdict; a save
   that dirtied nothing logs so and publishes nothing new. *)
let wait_for_verdict ic uri : Yojson.Safe.t list =
  let rec until_log () =
    match read_frame ic with
    | None -> failwith "the server closed before the binding check reported"
    | Some f -> (
        match binding_log f with
        | Some m -> ends_with ~suffix:"nothing to run" m
        | None -> until_log ())
  in
  if until_log () then []
  else
    let rec until_publish () =
      match read_frame ic with
      | None -> failwith "the server closed before publishing the verdict"
      | Some f -> (
          match publish_for uri f with
          | Some ds -> ds
          | None -> until_publish ())
    in
    until_publish ()

let replace_first ~a ~b s =
  let n = String.length s and m = String.length a in
  let rec find i =
    if i + m > n then None
    else if String.sub s i m = a then Some i
    else find (i + 1)
  in
  match find 0 with
  | Some i -> Some (String.sub s 0 i ^ b ^ String.sub s (i + m) (n - i - m))
  | None -> None

(* A whitespace toggle inside the first language block: the pair's own
   region changes, nothing another language crosses does. *)
let toggled (text : string) : string =
  match
    List.find_map
      (fun lang -> replace_first ~a:(lang ^ " {") ~b:(lang ^ " {   ") text)
      [ "go"; "ts"; "typescript"; "rust" ]
  with
  | Some t -> t
  | None -> failwith "the file has no language block to toggle"

let percentile (sorted : float array) (p : float) : float =
  let n = Array.length sorted in
  let rank = int_of_float (Float.ceil (p *. float_of_int n)) in
  sorted.(max 0 (min (n - 1) (rank - 1)))

let () =
  let server, path, latency =
    match Array.to_list Sys.argv with
    | [ _; server; path ] -> (server, path, None)
    | [ _; server; path; "--latency"; n ] ->
        (server, path, Some (int_of_string n))
    | _ ->
        prerr_endline
          "usage: lsp_check_driver <tono_lsp.exe> <file.tono> [--latency N]";
        exit 2
  in
  let path =
    if Filename.is_relative path then Filename.concat (Sys.getcwd ()) path
    else path
  in
  let text = In_channel.with_open_bin path In_channel.input_all in
  let uri = "file://" ^ path in
  let ic, oc = Unix.open_process_args server [| server |] in
  send oc
    {|{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}|};
  send oc
    (Printf.sprintf
       {|{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":%s,"languageId":"tono","version":1,"text":%s}}}|}
       (json_string uri) (json_string text));
  let save () =
    send oc
      (Printf.sprintf
         {|{"jsonrpc":"2.0","method":"textDocument/didSave","params":{"textDocument":{"uri":%s}}}|}
         (json_string uri))
  in
  (match latency with
  | None ->
      save ();
      let ds = wait_for_verdict ic uri in
      List.iter
        (fun d ->
          match BC.report_line (Diagnostic.t_of_yojson d) with
          | Some line -> print_endline line
          | None -> ())
        ds
  | Some n ->
      (* The first save primes every pair; the timed saves each dirty one. *)
      save ();
      ignore (wait_for_verdict ic uri);
      let samples =
        List.init n (fun i ->
            let edited = if i mod 2 = 0 then toggled text else text in
            send oc
              (Printf.sprintf
                 {|{"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":%s,"version":%d},"contentChanges":[{"text":%s}]}}|}
                 (json_string uri) (i + 2) (json_string edited));
            (* The frontend's publish for the change comes first; the
               verdict is what follows the save. *)
            let t0 = Unix.gettimeofday () in
            save ();
            ignore (wait_for_verdict ic uri);
            let ms = (Unix.gettimeofday () -. t0) *. 1000. in
            Printf.printf "save %2d: %.0f ms\n%!" (i + 1) ms;
            ms)
      in
      let sorted = Array.of_list samples in
      Array.sort compare sorted;
      Printf.printf "p50 %.0f ms, p95 %.0f ms over %d saves\n"
        (percentile sorted 0.5) (percentile sorted 0.95) n);
  send oc {|{"jsonrpc":"2.0","id":99,"method":"shutdown"}|};
  send oc {|{"jsonrpc":"2.0","method":"exit"}|};
  ignore (Unix.close_process (ic, oc))
