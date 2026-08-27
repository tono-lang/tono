(* The library index as the editor keeps it current: read from the file
   `tono index` writes beside the manifest, served to completion while its
   key matches the project as it is now, and rebuilt by the command itself
   (`tono index <file> --json --only <ext>=<lang>`) off the request thread
   when it is missing or stale.

   The hot path never waits on a process: a completion request reads the
   file (parsed once per version of it, by size and mtime) and checks the
   key; a build runs in a worker and the next request sees its result. A
   pair the builder could not index keeps its reason here, shown on the
   hover of the language word, and is retried on the next save, not on
   every request. *)

module FI = Tono_lsp_lib.Foreign_index
module BC = Tono_lsp_lib.Binding_check
module AF = Tono_lsp_lib.Analysis_foreign
module Analysis = Tono_lsp_lib.Analysis

(* --- state, shared between the request thread and the workers --- *)

let lock = Mutex.create ()

(* index path -> (mtime, size, what parsing it gave). *)
let loaded : (string, float * int * (FI.t, string) result) Hashtbl.t =
  Hashtbl.create 8

(* index path -> (the build's own key, why it produced no index). *)
let outcomes : (string, string * string) Hashtbl.t = Hashtbl.create 8

(* Index paths being built right now. *)
let in_flight : (string, unit) Hashtbl.t = Hashtbl.create 8
let workers : Thread.t list ref = ref []

let locked f =
  Mutex.lock lock;
  Fun.protect ~finally:(fun () -> Mutex.unlock lock) f

(* --- where the index of a pair is and what it says --- *)

let index_path ~(manifest_dir : string) ~(ext : string) ~(lang : string) :
    string =
  Filename.concat
    (Filename.concat (Filename.concat manifest_dir ".tono") "index")
    (ext ^ "." ^ lang ^ ".json")

let read_opt (path : string) : string option =
  if path = "" then None
  else
    try Some (In_channel.with_open_bin path In_channel.input_all)
    with Sys_error _ -> None

(* The file at [path] parsed, from the cache when its size and mtime are
   the ones cached (a rebuild is a rename over the file, so both move). *)
let load (path : string) : (FI.t, string) result option =
  match Unix.stat path with
  | exception Unix.Unix_error _ -> None
  | st -> (
      let stamp = (st.Unix.st_mtime, st.Unix.st_size) in
      match locked (fun () -> Hashtbl.find_opt loaded path) with
      | Some (m, s, parsed) when (m, s) = stamp -> Some parsed
      | _ ->
          let parsed =
            match read_opt path with
            | Some text -> FI.of_string text
            | None -> Error "index unreadable"
          in
          locked (fun () ->
              Hashtbl.replace loaded path (fst stamp, snd stamp, parsed));
          Some parsed)

(* The pair's index for the project as it is now, and whether a build is
   wanted: a missing or stale index asks for one, a pair the manifest does
   not pin or the ext does not declare for that language does not. *)
let status ~(manifest_dir : string) ~(manifest : string)
    ~(file : Tono_frontend.Ast.file) ~(ext : string) ~(lang : string) :
    FI.status * bool =
  match AF.package_of file ~ext ~lang with
  | None ->
      (FI.Missing (Printf.sprintf "the ext declares no %s path" lang), false)
  | Some package -> (
      match BC.version_in_manifest ~manifest { BC.ext; lang } with
      | None ->
          ( FI.Missing
              (Printf.sprintf "no %s version pinned in [ext.%s] of tono.toml"
                 lang ext),
            false )
      | Some version -> (
          let path = index_path ~manifest_dir ~ext ~lang in
          let build_key = package ^ "\000" ^ version in
          match load path with
          | None -> (
              match locked (fun () -> Hashtbl.find_opt outcomes path) with
              | Some (k, reason) when k = build_key -> (FI.Missing reason, true)
              | _ -> (FI.Missing "index not built yet", true))
          | Some (Error reason) -> (FI.Missing reason, true)
          | Some (Ok t) ->
              let expected =
                FI.expected_key ~ext ~lang ~package ~version
                  ~lockfile_path:t.key.lockfile_path
                  ~lockfile:(read_opt t.key.lockfile_path)
              in
              if FI.key_matches t expected then (FI.Ready t, false)
              else
                ( FI.Missing
                    "index is stale (the version or the lockfile changed); \
                     rebuilding",
                  true )))

(* The manifest governing [path]: its directory and text. *)
let manifest_of (path : string) : (string * string) option =
  match Binding_runner.manifest_path_for path with
  | None -> None
  | Some m -> (
      match read_opt m with
      | Some text -> Some (Filename.dirname m, text)
      | None -> None)

(* The lookup a request hands to the analysis: the manifest resolved once
   per request, the index read per pair. *)
let lookup ~(path : string) ~(file : Tono_frontend.Ast.file) : FI.lookup =
  let manifest = lazy (manifest_of path) in
  fun ~ext ~lang ->
    match Lazy.force manifest with
    | None -> FI.Missing "no tono.toml above the file"
    | Some (manifest_dir, manifest) ->
        fst (status ~manifest_dir ~manifest ~file ~ext ~lang)

(* --- building --- *)

let outcome_of_line (line : string) : (string * string * string) option =
  match Yojson.Safe.from_string line with
  | `Assoc fields -> (
      let str k =
        match List.assoc_opt k fields with Some (`String s) -> s | _ -> ""
      in
      match str "kind" with
      | "built" -> Some (str "ext", str "lang", "")
      | "skipped" -> Some (str "ext", str "lang", str "reason")
      | "error" -> Some ("", "", str "message")
      | _ -> None)
  | _ -> None
  | exception Yojson.Json_error _ -> None

(* Build the pairs of [path] (as [text] declares them) whose index is
   missing or stale. A pair a previous build could not index is retried
   only with [force] (a save), so a missing toolchain is not tried on every
   open. [log] gets one line per run. *)
let schedule ~(path : string) ~(text : string) ~(force : bool)
    ~(log : string -> unit) ~(on_done : unit -> unit) : unit =
  match manifest_of path with
  | None -> ()
  | Some (manifest_dir, manifest) ->
      let file = Analysis.parse text in
      let wanted =
        List.filter_map
          (fun (p : BC.pair) ->
            let index = index_path ~manifest_dir ~ext:p.ext ~lang:p.lang in
            let _, build =
              status ~manifest_dir ~manifest ~file ~ext:p.ext ~lang:p.lang
            in
            let tried = locked (fun () -> Hashtbl.mem outcomes index) in
            if build && (force || not tried) then Some (p, index) else None)
          (BC.pairs file)
      in
      let dirty =
        locked (fun () ->
            let dirty =
              List.filter
                (fun (_, index) -> not (Hashtbl.mem in_flight index))
                wanted
            in
            List.iter
              (fun (_, index) -> Hashtbl.replace in_flight index ())
              dirty;
            dirty)
      in
      if dirty <> [] then begin
        let base = Filename.basename path in
        let work () =
          let t0 = Unix.gettimeofday () in
          let bin = Binding_runner.tono_bin () in
          let started =
            List.map
              (fun ((p : BC.pair), index) ->
                let argv =
                  [|
                    bin; "index"; path; "--json"; "--only"; p.ext ^ "=" ^ p.lang;
                  |]
                in
                (p, index, Binding_runner.spawn ~bin ~argv))
              dirty
          in
          let outcomes_now =
            List.map
              (fun ((p : BC.pair), index, r) ->
                let lines =
                  match r with
                  | Ok run -> Binding_runner.collect ~what:"tono index" run
                  | Error message -> [ Binding_runner.error_line message ]
                in
                let reason =
                  List.fold_left
                    (fun acc line ->
                      match outcome_of_line line with
                      | Some (_, _, "") -> None
                      | Some (_, _, reason) -> Some reason
                      | None -> acc)
                    (Some "tono index printed no report") lines
                in
                (p, index, reason))
              started
          in
          locked (fun () ->
              List.iter
                (fun ((p : BC.pair), index, reason) ->
                  Hashtbl.remove in_flight index;
                  Hashtbl.remove loaded index;
                  match reason with
                  | None -> Hashtbl.remove outcomes index
                  | Some reason ->
                      let key =
                        match
                          ( AF.package_of file ~ext:p.ext ~lang:p.lang,
                            BC.version_in_manifest ~manifest p )
                        with
                        | Some package, Some version ->
                            package ^ "\000" ^ version
                        | _ -> ""
                      in
                      Hashtbl.replace outcomes index (key, reason))
                outcomes_now);
          let built =
            List.filter (fun (_, _, reason) -> reason = None) outcomes_now
          in
          log
            (Printf.sprintf "index %s: %d of %d pairs built (%s), %.0fms" base
               (List.length built) (List.length dirty)
               (String.concat ", "
                  (List.map
                     (fun ((p : BC.pair), _, reason) ->
                       p.ext ^ "/" ^ p.lang
                       ^ match reason with None -> "" | Some r -> ": " ^ r)
                     outcomes_now))
               ((Unix.gettimeofday () -. t0) *. 1000.));
          on_done ()
        in
        let guarded () =
          try work ()
          with e ->
            locked (fun () ->
                List.iter
                  (fun (_, index) -> Hashtbl.remove in_flight index)
                  dirty);
            log
              (Printf.sprintf "index %s: failed (%s)" base
                 (Printexc.to_string e))
        in
        workers := Thread.create guarded () :: !workers
      end

(* Let every build in progress finish: on exit, so the index a save asked
   for is on disk for the next session. *)
let wait_all () : unit =
  List.iter Thread.join !workers;
  workers := []
