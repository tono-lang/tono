(* Server-side workspace state: root discovery on disk, project scanning, the
   open-buffer overlay, and the per-module diagnostics cache. Everything
   semantic lives in the pure [Analysis]; this module owns only the IO around
   it. *)

module Analysis = Tono_lsp_lib.Analysis

(* The source root governing [path]: a directory carrying tono.toml wins (its
   src/ child when present, itself otherwise), else the nearest ancestor
   directory literally named src (the module system's layout convention).
   None means single-file mode. *)
let source_root_of (path : string) : string option =
  let rec up dir src_seen =
    let manifest = Filename.concat dir "tono.toml" in
    if Sys.file_exists manifest then
      let src = Filename.concat dir "src" in
      if Sys.file_exists src && Sys.is_directory src then Some src else Some dir
    else
      let src_seen =
        if src_seen = None && Filename.basename dir = "src" then Some dir
        else src_seen
      in
      let parent = Filename.dirname dir in
      if String.equal parent dir then src_seen else up parent src_seen
  in
  up (Filename.dirname path) None

(* Every .tono file under [root] as a relative path, sorted so module order is
   stable (matching compile-dir). *)
let scan (root : string) : string list =
  let rec walk rel acc =
    let dir = if rel = "" then root else Filename.concat root rel in
    match Sys.readdir dir with
    | exception Sys_error _ -> acc
    | entries ->
        Array.fold_left
          (fun acc entry ->
            let rel' = if rel = "" then entry else Filename.concat rel entry in
            let full = Filename.concat root rel' in
            if try Sys.is_directory full with Sys_error _ -> false then
              walk rel' acc
            else if Filename.check_suffix entry ".tono" then rel' :: acc
            else acc)
          acc entries
  in
  List.sort compare (walk "" [])

(* The module name of an absolute path under [root], through the same rel-path
   mapping the CLI's compile-dir uses. *)
let module_of ~(root : string) (path : string) : string option =
  let prefix = root ^ Filename.dir_sep in
  let lp = String.length prefix in
  if String.length path > lp && String.equal (String.sub path 0 lp) prefix then
    Some
      (Tono_frontend.Cli.module_name_of_rel
         (String.sub path lp (String.length path - lp)))
  else None

(* Parse cache: an entry is rebuilt only when its text changed, so a project
   rebuild after one edit re-parses one file. *)
let parse_cache : (string, string * Analysis.project_entry) Hashtbl.t =
  Hashtbl.create 64

let entry_for ~(overlay : string -> string option) ~(root : string)
    (rel : string) : Analysis.project_entry option =
  let full = Filename.concat root rel in
  let text =
    match overlay full with
    | Some t -> Some t
    | None -> (
        try Some (In_channel.with_open_bin full In_channel.input_all)
        with Sys_error _ -> None)
  in
  Option.map
    (fun text ->
      match Hashtbl.find_opt parse_cache full with
      | Some (t, e) when String.equal t text -> e
      | _ ->
          let module_ = Tono_frontend.Cli.module_name_of_rel rel in
          let id = Lsp.Uri.to_string (Lsp.Uri.of_path full) in
          let e = Analysis.project_entry ~module_ ~id ~text in
          Hashtbl.replace parse_cache full (text, e);
          e)
    text

(* The whole project under [root], open-buffer contents taking precedence over
   disk. *)
let project_of ~(overlay : string -> string option) (root : string) :
    Analysis.project =
  Analysis.build_project
    (List.filter_map (entry_for ~overlay ~root) (scan root))

(* Diagnostics cache keyed by the module's content closure: an edit re-checks
   only the edited module and the open modules that (transitively) import it;
   everything else is a key hit. *)
let diag_cache : (string, string * Lsp.Types.Diagnostic.t list) Hashtbl.t =
  Hashtbl.create 64

let diagnostics ~(project : Analysis.project) ~(root : string)
    ~(module_ : string) : Lsp.Types.Diagnostic.t list =
  let key = Analysis.module_check_key project ~module_ in
  let slot = root ^ "\x00" ^ module_ in
  match Hashtbl.find_opt diag_cache slot with
  | Some (k, ds) when String.equal k key -> ds
  | _ ->
      let ds = Analysis.project_diagnostics project ~module_ in
      Hashtbl.replace diag_cache slot (key, ds);
      ds
