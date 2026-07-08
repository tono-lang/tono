(* Project-level module machinery: the per-module symbol tables, the import maps,
   and the cross-module reference resolution built on top of [Symtab]. A project
   is a set of named modules (the name is the file path, dotted); references
   between them go through an import, and the module import graph must be a DAG.

   Two consumers read this index: lowering, which turns a reference into a
   fully-qualified "module#name" id ([resolver]), and the typechecker, which
   reports unresolved imports and non-[pub] references ([qualified]). Lowering
   never diagnoses; the typechecker never renames. *)

module SMap = Map.Make (String)

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt

(* A diagnostic carries a span but no file identity, so in a multi-module project
   the owning module is prepended to its message for attribution. *)
let label (name : string) (d : Diagnostic.t) : Diagnostic.t =
  { d with message = name ^ ": " ^ d.message }

(* The qualifier a reference uses for an import: its alias when given, otherwise
   the last path segment. [import payments.common] is referenced as [common];
   [import payments.common as c] as [c]. *)
let qualifier_of (i : Ast.import) : string =
  match i.Ast.alias with
  | Some a -> a
  | None -> ( match List.rev i.Ast.imported_path with x :: _ -> x | [] -> "")

(* The target module name of an import: the dotted path. *)
let target_of (i : Ast.import) : string = String.concat "." i.Ast.imported_path

type index = {
  symbols : Symtab.t SMap.t; (* module name -> its symbol table *)
  imports : Ast.import list SMap.t; (* module name -> its imports *)
}

(* The qualifier -> target-module map for one module (last import of a qualifier
   wins). *)
let import_map (index : index) (this_module : string) : string SMap.t =
  match SMap.find_opt this_module index.imports with
  | None -> SMap.empty
  | Some imports ->
      List.fold_left
        (fun m i -> SMap.add (qualifier_of i) (target_of i) m)
        SMap.empty imports

(* ── Cycle detection (the module import graph must be a DAG) ────────────── *)

(* A module's imported targets that name a real module, paired with the import
   span so a cycle diagnostic can point at the edge that closes it. *)
let edges (index : index) (m : string) : (string * Span.span) list =
  match SMap.find_opt m index.imports with
  | None -> []
  | Some imports ->
      List.filter_map
        (fun (i : Ast.import) ->
          let target = target_of i in
          if SMap.mem target index.symbols then Some (target, i.Ast.ispan)
          else None)
        imports

let detect_cycles (index : index) : Diagnostic.t list =
  let done_ = Hashtbl.create 16 in
  let reported = Hashtbl.create 16 in
  let diags = ref [] in
  let report owner members span =
    (* [members] is the cycle in reverse discovery order; dedupe by its set so
       one cycle is reported once regardless of the entry point. *)
    let key = String.concat "\x00" (List.sort compare members) in
    if not (Hashtbl.mem reported key) then (
      Hashtbl.add reported key ();
      let ring = List.rev members in
      let path = String.concat " -> " (ring @ [ List.hd ring ]) in
      diags :=
        label owner
          (err Error_codes.module_cycle span
             "module import cycle (imports must form a DAG): %s" path)
        :: !diags)
  in
  let rec dfs stack m =
    List.iter
      (fun (target, span) ->
        if String.equal target m || List.mem target stack then
          (* [m] imports [target], which is [m] itself or an ancestor: a cycle.
             Its members are [m] plus the ancestors back up to [target]. *)
          let rec upto = function
            | x :: _ when String.equal x target -> [ x ]
            | x :: rest -> x :: upto rest
            | [] -> []
          in
          report m (m :: upto stack) span
        else if not (Hashtbl.mem done_ target) then dfs (m :: stack) target)
      (edges index m);
    Hashtbl.replace done_ m ()
  in
  SMap.iter
    (fun m _ -> if not (Hashtbl.mem done_ m) then dfs [] m)
    index.symbols;
  List.rev !diags

(* ── Building the index ────────────────────────────────────────────────── *)

(* Build the index from every module's parsed file, collecting per-module
   duplicate-shape diagnostics, flagging imports whose target module does not
   exist (TC0023), and detecting import cycles (TC0025). *)
let build (files : (string * Ast.file) list) : index * Diagnostic.t list =
  let symbols, imports, dup_diags =
    List.fold_left
      (fun (symbols, imports, diags) (name, (file : Ast.file)) ->
        let tbl, dups = Symtab.build file.Ast.decls in
        ( SMap.add name tbl symbols,
          SMap.add name file.Ast.imports imports,
          diags @ List.map (label name) dups ))
      (SMap.empty, SMap.empty, [])
      files
  in
  let index = { symbols; imports } in
  let import_diags =
    SMap.fold
      (fun name imports acc ->
        List.fold_left
          (fun acc (i : Ast.import) ->
            let target = target_of i in
            if SMap.mem target index.symbols then acc
            else
              label name
                (err Error_codes.unknown_import i.Ast.ispan
                   "unknown module '%s'; no such file in the project" target)
              :: acc)
          acc imports)
      index.imports []
  in
  (index, dup_diags @ import_diags @ detect_cycles index)

(* ── Resolution used by lowering and the typechecker ───────────────────── *)

(* The reference resolver lowering uses to qualify ids: an in-module name takes
   this module's prefix; a qualified reference follows the import map. An unknown
   qualifier falls back to using it as a module directly, which the [qualified]
   check below turns into a diagnostic. *)
let resolver (index : index) ~(this_module : string) : Lower.ref_resolver =
  let imap = import_map index this_module in
  fun ~qualifier ~name ->
    match qualifier with
    | None -> Lower.qualify this_module name
    | Some q -> (
        match SMap.find_opt q imap with
        | Some target -> Lower.qualify target name
        | None -> Lower.qualify q name)

(* Arity check for a resolved cross-module reference, mirroring the in-module
   check in [Resolve]. *)
let arity_diags name target arity n_args span : Diagnostic.t list =
  if n_args = arity then []
  else if arity = 0 then
    [
      err Error_codes.non_generic_applied span
        "'%s.%s' is not generic and takes no type arguments" target name;
    ]
  else
    [
      err Error_codes.generic_arity_mismatch span
        "'%s.%s' expects %d type argument(s), but %d %s given" target name arity
        n_args
        (if n_args = 1 then "was" else "were");
    ]

(* The cross-module reference check the typechecker plugs into [Resolve]: a
   qualifier must be imported (else TC0023), the referenced shape must exist and
   be [pub] in the target module (else TC0024), and its generic arity must
   match. *)
let qualified (index : index) ~(this_module : string) : Resolve.qualified =
  let imap = import_map index this_module in
  fun ~qualifier ~name ~n_args span ->
    match SMap.find_opt qualifier imap with
    | None ->
        [
          err Error_codes.unknown_import span
            "unknown module qualifier '%s'; import the module to reference it"
            qualifier;
        ]
    | Some target -> (
        match SMap.find_opt target index.symbols with
        (* The import named a missing module; that is already reported at the
         import, so the reference itself stays quiet. *)
        | None -> []
        | Some tbl -> (
            match Symtab.find name tbl with
            | None ->
                [
                  err Error_codes.unknown_type span
                    "unknown type '%s' in module '%s'" name target;
                ]
            | Some { pub = false; _ } ->
                [
                  err Error_codes.not_exported span
                    "'%s' is not exported from module '%s'; mark it 'pub' to \
                     reference it across modules"
                    name target;
                ]
            | Some { pub = true; arity; _ } ->
                arity_diags name target arity n_args span))
