(* The binding check as the editor runs it: on save, each dirty (ext,
   language) pair of the saved file is checked by the `tono` command itself
   (`tono check <file> --json --only <ext>=<lang>`), the pairs in parallel,
   off the request thread, and the verdict is published when it lands.

   There is one checker: what this publishes is what `tono check` prints,
   read from the same process, never re-derived here. The command costs
   milliseconds to start, so a process per pair is the whole bridge, with no
   daemon to keep in step.

   The verdict of a pair is cached under its key ([Binding_check.key]) and a
   save re-runs only the pairs whose key changed and is not already being
   checked: a second save of the same text costs nothing. Cached verdicts
   are re-located onto the current text when published, so a pair's finding
   keeps pointing at its binding while another pair is being re-checked. *)

open Lsp.Types
module BC = Tono_lsp_lib.Binding_check
module Analysis = Tono_lsp_lib.Analysis

(* The `tono` command: [TONO_BIN], a sibling of this executable (an
   installed pair), the cargo build tree above a dune build tree (a dev
   checkout), else `tono` on PATH. *)
let tono_bin () : string =
  match Sys.getenv_opt "TONO_BIN" with
  | Some p when p <> "" -> p
  | _ ->
      let here = Filename.dirname Sys.executable_name in
      let rec ancestors dir acc =
        let parent = Filename.dirname dir in
        if String.equal parent dir then List.rev (dir :: acc)
        else ancestors parent (dir :: acc)
      in
      let candidates =
        Filename.concat here "tono"
        :: List.concat_map
             (fun dir ->
               [
                 Filename.concat dir "target/debug/tono";
                 Filename.concat dir "target/release/tono";
               ])
             (ancestors here [])
      in
      Option.value ~default:"tono" (List.find_opt Sys.file_exists candidates)

(* The nearest tono.toml above [path], read whole; None without one. *)
let manifest_for (path : string) : string option =
  let rec up dir =
    let candidate = Filename.concat dir "tono.toml" in
    if Sys.file_exists candidate then
      try Some (In_channel.with_open_bin candidate In_channel.input_all)
      with Sys_error _ -> None
    else
      let parent = Filename.dirname dir in
      if String.equal parent dir then None else up parent
  in
  up (Filename.dirname path)

(* --- state, shared between the request thread and the workers --- *)

let lock = Mutex.create ()

(* (path, ext, lang) -> the key the verdict was produced under, its lines. *)
let results : (string * string * string, string * string list) Hashtbl.t =
  Hashtbl.create 16

(* Keys being checked right now. *)
let in_flight : (string, unit) Hashtbl.t = Hashtbl.create 16
let workers : Thread.t list ref = ref []

let locked f =
  Mutex.lock lock;
  Fun.protect ~finally:(fun () -> Mutex.unlock lock) f

(* --- one pair, one process --- *)

let error_line (message : string) : string =
  Yojson.Safe.to_string
    (`Assoc [ ("kind", `String "error"); ("message", `String message) ])

type running = { pid : int; out : Unix.file_descr; err_path : string }

(* Start the check for one pair. Its stdin is /dev/null (the editor's stdin
   is the protocol stream, not the child's); stderr goes to a file so
   reading stdout to the end can never block on a full stderr pipe. *)
let spawn ~(bin : string) ~(path : string) (p : BC.pair) :
    (running, string) result =
  let err_path = Filename.temp_file "tono-lsp-check" ".err" in
  match
    let err_fd = Unix.openfile err_path [ Unix.O_WRONLY; Unix.O_TRUNC ] 0o600 in
    let devnull = Unix.openfile "/dev/null" [ Unix.O_RDONLY ] 0 in
    let r, w = Unix.pipe ~cloexec:true () in
    let argv =
      [| bin; "check"; path; "--json"; "--only"; p.ext ^ "=" ^ p.lang |]
    in
    let pid =
      Fun.protect
        ~finally:(fun () ->
          Unix.close w;
          Unix.close err_fd;
          Unix.close devnull)
        (fun () ->
          try Unix.create_process bin argv devnull w err_fd
          with e ->
            Unix.close r;
            raise e)
    in
    { pid; out = r; err_path }
  with
  | running -> Ok running
  | exception Unix.Unix_error (e, _, _) ->
      (try Sys.remove err_path with Sys_error _ -> ());
      Error
        (Printf.sprintf "could not run %s (%s); set TONO_BIN to the tono binary"
           bin (Unix.error_message e))

(* Wait for the check and read its report. A run that printed no report and
   failed is reported through its last stderr line, so a check that could
   not even start is a diagnostic, never silence. *)
let collect (r : running) : string list =
  let ic = Unix.in_channel_of_descr r.out in
  let out = In_channel.input_all ic in
  close_in ic;
  let _, status = Unix.waitpid [] r.pid in
  let err =
    try In_channel.with_open_bin r.err_path In_channel.input_all
    with Sys_error _ -> ""
  in
  (try Sys.remove r.err_path with Sys_error _ -> ());
  let lines =
    List.filter (fun l -> String.trim l <> "") (String.split_on_char '\n' out)
  in
  match status with
  | Unix.WEXITED 0 -> lines
  | _ when lines <> [] -> lines
  | status ->
      let last =
        List.fold_left
          (fun acc l -> if String.trim l = "" then acc else String.trim l)
          ""
          (String.split_on_char '\n' err)
      in
      let how =
        match status with
        | Unix.WEXITED c -> Printf.sprintf "exited with code %d" c
        | Unix.WSIGNALED s | Unix.WSTOPPED s ->
            Printf.sprintf "stopped by signal %d" s
      in
      [
        error_line
          (if last = "" then "tono check " ^ how ^ " and printed no report"
           else "tono check " ^ how ^ ": " ^ last);
      ]

(* --- the save-time entry point --- *)

let pair_name (p : BC.pair) = p.ext ^ "/" ^ p.lang

(* Check the dirty pairs of [path] as saved with [text], then call [on_done]
   from the worker once every verdict is stored. [log] receives one line
   per save naming what ran and how long it took. Nothing runs for a file
   the frontend rejects: the binding check needs a source it accepts. *)
let schedule ~(path : string) ~(text : string) ~(clean : bool)
    ~(log : string -> unit) ~(on_done : unit -> unit) : unit =
  let file = Analysis.parse text in
  let pairs = BC.pairs file in
  if clean && pairs <> [] then begin
    let manifest = manifest_for path in
    let keyed = List.map (fun p -> (p, BC.key ~text ~manifest file p)) pairs in
    let dirty =
      locked (fun () ->
          let dirty =
            List.filter
              (fun ((p : BC.pair), k) ->
                (not (Hashtbl.mem in_flight k))
                &&
                match Hashtbl.find_opt results (path, p.ext, p.lang) with
                | Some (k', _) -> not (String.equal k k')
                | None -> true)
              keyed
          in
          List.iter (fun (_, k) -> Hashtbl.replace in_flight k ()) dirty;
          dirty)
    in
    let total = List.length pairs in
    let base = Filename.basename path in
    if dirty = [] then
      log
        (Printf.sprintf "binding check %s: 0 of %d pairs dirty, nothing to run"
           base total)
    else
      let work () =
        let t0 = Unix.gettimeofday () in
        let bin = tono_bin () in
        let started =
          List.map (fun (p, k) -> (p, k, spawn ~bin ~path p)) dirty
        in
        let outcomes =
          List.map
            (fun (p, k, r) ->
              ( p,
                k,
                match r with
                | Ok run -> collect run
                | Error message -> [ error_line message ] ))
            started
        in
        locked (fun () ->
            List.iter
              (fun ((p : BC.pair), k, lines) ->
                Hashtbl.replace results (path, p.ext, p.lang) (k, lines);
                Hashtbl.remove in_flight k)
              outcomes);
        log
          (Printf.sprintf
             "binding check %s: %d of %d pairs checked (%s), %.0fms" base
             (List.length dirty) total
             (String.concat ", " (List.map (fun (p, _) -> pair_name p) dirty))
             ((Unix.gettimeofday () -. t0) *. 1000.));
        on_done ()
      in
      let guarded () =
        try work ()
        with e ->
          locked (fun () ->
              List.iter (fun (_, k) -> Hashtbl.remove in_flight k) dirty);
          log
            (Printf.sprintf "binding check %s: failed (%s)" base
               (Printexc.to_string e))
      in
      workers := Thread.create guarded () :: !workers
  end

(* The cached verdicts of [path]'s pairs over [text] as it is now. *)
let diagnostics ~(path : string) ~(text : string) : Diagnostic.t list =
  let file = Analysis.parse text in
  let pairs = BC.pairs file in
  let cached =
    locked (fun () ->
        List.filter_map
          (fun (p : BC.pair) ->
            Option.map
              (fun (_, lines) -> (p, lines))
              (Hashtbl.find_opt results (path, p.ext, p.lang)))
          pairs)
  in
  List.concat_map
    (fun (p, lines) -> BC.diagnostics_of_lines ~text ~file p lines)
    cached

let forget ~(path : string) : unit =
  locked (fun () ->
      let stale =
        Hashtbl.fold
          (fun ((p, _, _) as k) _ acc ->
            if String.equal p path then k :: acc else acc)
          results []
      in
      List.iter (Hashtbl.remove results) stale)

(* Let every check in progress finish and publish: on exit, so a client
   that sent a save and then exit still gets the verdict. *)
let wait_all () : unit =
  List.iter Thread.join !workers;
  workers := []
