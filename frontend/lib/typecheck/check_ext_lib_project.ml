(* Cross-file closed accounting for the "ext"/"extern" FFI library block
   (decision K): the same "ext" name's module path for one language declared
   with two different targets (TC0079); an extern (or opaque-type method)
   name repeated within one "ext", even across files (TC0080); a language
   block for a target the "ext" declares no module path for (TC0081). Split
   out of [Check_ext_lib] to keep that file under the line-count cap; the
   per-extern, per-language-binding checks (TC0070-TC0078) stay there. *)

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt

type occurrence = { file : string; body : Ast.ext_lib_body }

(* Every ext-block occurrence, grouped by its declared name, across every
   named decl-list (one entry per file, or a single "" entry for a lone
   module). *)
let occurrences_by_name (files : (string * Ast.decl list) list) :
    (string, occurrence list) Hashtbl.t =
  let groups = Hashtbl.create 8 in
  List.iter
    (fun (file, decls) ->
      List.iter
        (fun (d : Ast.decl) ->
          match d.Ast.dkind with
          | Ast.DExtLib { body; _ } ->
              let prev =
                Option.value ~default:[] (Hashtbl.find_opt groups d.Ast.dname)
              in
              Hashtbl.replace groups d.Ast.dname ({ file; body } :: prev)
          | _ -> ())
        decls)
    files;
  groups

(* A language's module path declared with two different targets across the
   group is a conflict; every declaring occurrence is flagged, each labeled
   with its own file. A repeated declaration of the *same* path is not an
   error, only a conflicting one is. *)
let check_module_paths (ext_name : string) (occs : occurrence list) :
    (string * Diagnostic.t) list =
  let entries =
    List.concat_map
      (fun occ ->
        List.map
          (fun (lp : Ast.lang_path) ->
            (lp.Ast.lp_lang, lp.Ast.lp_path, lp.Ast.lp_lang_span, occ.file))
          occ.body.Ast.elib_langs)
      occs
  in
  let langs =
    List.sort_uniq compare (List.map (fun (l, _, _, _) -> l) entries)
  in
  List.concat_map
    (fun lang ->
      let group =
        List.filter (fun (l, _, _, _) -> String.equal l lang) entries
      in
      let paths =
        List.sort_uniq compare (List.map (fun (_, p, _, _) -> p) group)
      in
      if List.length paths <= 1 then []
      else
        (* Every other distinct (path, file) pair in the group, named in
           full so one diagnostic is enough to see both sides of the
           conflict without diffing it against a second message. *)
        let describe path file = Printf.sprintf "'%s' (in '%s')" path file in
        List.map
          (fun (_, path, span, file) ->
            let others =
              List.sort_uniq compare
                (List.filter_map
                   (fun (_, p, _, f) ->
                     if String.equal p path && String.equal f file then None
                     else Some (describe p f))
                   group)
            in
            ( file,
              err Error_codes.ext_lib_module_path_conflict span
                "module path for '%s' in ext '%s' is declared as %s here, \
                 conflicting with %s"
                lang ext_name (describe path file)
                (String.concat ", " others) ))
          group)
    langs

(* A name repeated within one namespace (the ext's free externs, or one
   opaque type's own methods) is a conflict, even across files; the first
   declaration wins silently, every later one is flagged. *)
let dup_name_diags ~code (items : (string * Span.span * string) list) :
    (string * Diagnostic.t) list =
  let seen = Hashtbl.create 8 in
  List.filter_map
    (fun (name, span, file) ->
      if Hashtbl.mem seen name then
        Some (file, err code span "'%s' is already declared" name)
      else (
        Hashtbl.add seen name ();
        None))
    items

let check_duplicate_names (occs : occurrence list) :
    (string * Diagnostic.t) list =
  let frees =
    List.concat_map
      (fun occ ->
        List.map
          (fun (e : Ast.extern_decl) ->
            (e.Ast.ed_name, e.Ast.ed_name_span, occ.file))
          occ.body.Ast.elib_externs)
      occs
  in
  let methods_by_type = Hashtbl.create 8 in
  List.iter
    (fun occ ->
      List.iter
        (fun (t : Ast.opaque_type) ->
          let prev =
            Option.value ~default:[]
              (Hashtbl.find_opt methods_by_type t.Ast.opq_name)
          in
          Hashtbl.replace methods_by_type t.Ast.opq_name
            (prev
            @ List.map
                (fun (m : Ast.extern_decl) ->
                  (m.Ast.ed_name, m.Ast.ed_name_span, occ.file))
                t.Ast.opq_methods))
        occ.body.Ast.elib_types)
    occs;
  dup_name_diags ~code:Error_codes.extern_duplicate_name frees
  @ List.concat_map
      (fun items ->
        dup_name_diags ~code:Error_codes.extern_duplicate_name items)
      (Hashtbl.fold (fun _ items acc -> items :: acc) methods_by_type [])

(* A language block bound in a call: for a target the ext declares no module
   path for is inert: nothing tells the generator where the symbol lives. *)
let check_lang_has_module (ext_name : string) (occs : occurrence list) :
    (string * Diagnostic.t) list =
  let declared_langs =
    List.sort_uniq compare
      (List.concat_map
         (fun occ ->
           List.map
             (fun (lp : Ast.lang_path) -> lp.Ast.lp_lang)
             occ.body.Ast.elib_langs)
         occs)
  in
  let externs_of occ =
    occ.body.Ast.elib_externs
    @ List.concat_map
        (fun (t : Ast.opaque_type) -> t.Ast.opq_methods)
        occ.body.Ast.elib_types
  in
  List.concat_map
    (fun occ ->
      List.concat_map
        (fun (e : Ast.extern_decl) ->
          List.filter_map
            (fun (b : Ast.extern_lang_body) ->
              if List.mem b.Ast.elb_lang declared_langs then None
              else
                Some
                  ( occ.file,
                    err Error_codes.extern_lang_no_module b.Ast.elb_lang_span
                      "language '%s' has no declared module path in ext '%s'"
                      b.Ast.elb_lang ext_name ))
            e.Ast.ed_langs)
        (externs_of occ))
    occs

let check_project (files : (string * Ast.decl list) list) : Diagnostic.t list =
  let groups = occurrences_by_name files in
  Hashtbl.fold
    (fun ext_name occs acc ->
      let attributed =
        check_module_paths ext_name occs
        @ check_duplicate_names occs
        @ check_lang_has_module ext_name occs
      in
      List.map
        (fun (file, d) ->
          if String.equal file "" then d
          else
            { d with Diagnostic.message = file ^ ": " ^ d.Diagnostic.message })
        attributed
      @ acc)
    groups []
