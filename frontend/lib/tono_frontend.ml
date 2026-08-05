(* OCaml frontend: lexer, parser, typecheck, IR. *)
let version = "0.0.0"

(* Re-export the submodules so consumers reach them as [Tono_frontend.Ir] etc.
   A custom main module suppresses dune's automatic submodule aliasing. *)
module Ir = Ir
module Ir_json = Ir_json
module Span = Span
module Diagnostic = Diagnostic
module Token = Token
module Lexer = Lexer
module Ast = Ast
module Parser_state = Parser_state
module Parser = Parser
module Printer = Printer
module Lower = Lower
module Typecheck = Typecheck
module Check_ext = Check_ext
module Check_entries = Check_entries
module Entry_scope = Entry_scope
module Protocol_http = Protocol_http
module Modules = Modules
module Error_codes = Error_codes
module Trait_vocab = Trait_vocab

(* The pure calculus: a self-contained, total expression sub-language. Its
   modules carry a [Calc_] prefix because the library namespace is flat. *)
module Calc_ast = Calc_ast
module Calc_token = Calc_token
module Calc_lexer = Calc_lexer
module Calc_parser = Calc_parser
module Calc_types = Calc_types
module Calc_check = Calc_check
module Calc_eval = Calc_eval

(* The frontend pipeline: lex and parse source text, lower it to an IR module,
   then typecheck that module. [module_name] names the resulting module. All lex,
   parse, lowering, and typecheck diagnostics are merged and returned in source
   order. *)
let compile ?(module_name = "") (src : string) : Ir.module_ * Diagnostic.t list
    =
  let file, parse_diags = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name ~diags file in
  let m, tc_diags = Typecheck.check_module ~file m in
  (m, Diagnostic.sort (parse_diags @ List.rev !diags @ tc_diags))

(* Compile a source string straight to canonical IR JSON. Errors (not warnings)
   abort with their messages joined; otherwise the single module is wrapped in a
   versioned model and encoded. This is the payload behind the [compile]
   subcommand, kept pure so it can be tested without touching the filesystem. *)
let compile_to_json ?(module_name = "") (src : string) : (string, string) result
    =
  let m, diags = compile ~module_name src in
  let errors =
    List.filter (fun (d : Diagnostic.t) -> d.severity = Diagnostic.Error) diags
  in
  if errors <> [] then
    Error (String.concat "\n" (List.map Diagnostic.to_string errors))
  else
    (* Protocol resolution is the final IR -> IR step: each operation's HTTP
       annotations are materialized into a typed [wire] field plus (for
       backward compatibility) an opaque wire_descriptor trait the backend
       embeds verbatim. *)
    let m = Protocol_http.resolve_module m in
    let model =
      { Ir.tono_ir_version = Ir_json.current_ir_version; modules = [ m ] }
    in
    Ok (Ir_json.to_canonical_string (Ir_json.encode_model model))

(* One module's lower+typecheck against a prebuilt project index: the
   per-module unit behind [compile_project], exposed so tooling (the LSP) can
   attribute diagnostics to files and cache per module without a second copy
   of the pipeline. The operation's HTTP annotations are materialized into the
   wire descriptor, mirroring the single-module path. *)
let check_project_module (index : Modules.index) ~(name : string)
    (file : Ast.file) : Ir.module_ * Diagnostic.t list =
  let resolve = Modules.resolver index ~this_module:name in
  let qualified = Modules.qualified index ~this_module:name in
  let ldiags = ref [] in
  let m = Lower.lower_file ~module_name:name ~resolve ~diags:ldiags file in
  let m, tdiags = Typecheck.check_module ~qualified ~file m in
  (Protocol_http.resolve_module m, List.rev !ldiags @ tdiags)

(* Compile a whole project: a set of (qualified module name, source) pairs, where
   the name is derived from the file path (payments/charge.tono ->
   payments.charge). Every module is parsed, then the project index resolves
   cross-module references, enforces two-level visibility, and rejects import
   cycles; finally each module is lowered with namespaced ids and typechecked.
   Diagnostics are prefixed with their module name for attribution across files. *)
let compile_project (files : (string * string) list) :
    Ir.model * Diagnostic.t list =
  let parsed =
    List.map
      (fun (name, src) ->
        let file, pdiags = Parser.parse src in
        (name, file, pdiags))
      files
  in
  let index, mod_diags =
    Modules.build (List.map (fun (n, f, _) -> (n, f)) parsed)
  in
  let label name (d : Diagnostic.t) =
    { d with Diagnostic.message = name ^ ": " ^ d.message }
  in
  (* Lower and typecheck every module even when [build] found import or cycle
     errors: resolution does not traverse the import graph, so a cycle does not
     derail it, and running the per-module checks surfaces every diagnostic in one
     pass rather than one class at a time. The model is discarded upstream whenever
     there is any error. *)
  let modules, per_diags =
    List.fold_right
      (fun (name, file, pdiags) (mods, diags) ->
        let m, own = check_project_module index ~name file in
        let here = List.map (label name) (pdiags @ own) in
        (m :: mods, here @ diags))
      parsed ([], [])
  in
  let model = { Ir.tono_ir_version = Ir_json.current_ir_version; modules } in
  (model, Diagnostic.sort (mod_diags @ per_diags))

(* Compile a project straight to canonical IR JSON, or the joined error messages
   when any module has an error. *)
let compile_project_to_json (files : (string * string) list) :
    (string, string) result =
  let model, diags = compile_project files in
  let errors =
    List.filter (fun (d : Diagnostic.t) -> d.severity = Diagnostic.Error) diags
  in
  if errors <> [] then
    Error (String.concat "\n" (List.map Diagnostic.to_string errors))
  else Ok (Ir_json.to_canonical_string (Ir_json.encode_model model))

(* Re-emit source text in the printer's canonical layout. Formatting is
   parse-level only (no lowering or typecheck), but parse errors abort: a file
   the parser had to recover on would be rewritten with the recovered guesses.
   This is the payload behind the [fmt] subcommand, pure for the same reason as
   [compile_to_json]. *)
let format_source (src : string) : (string, string) result =
  let file, diags = Parser.parse src in
  let errors =
    List.filter (fun (d : Diagnostic.t) -> d.severity = Diagnostic.Error) diags
  in
  if errors <> [] then
    Error (String.concat "\n" (List.map Diagnostic.to_string errors))
  else Ok (Printer.print_file file)

(* The [tono-frontend] command line. The dispatch is pure: it takes the argv and
   a file reader, and returns what to write where plus an exit code, so the real
   binary is a thin shell and every branch is testable. *)
module Cli = struct
  type outcome = { code : int; out : string; err : string }

  let usage =
    "usage: tono-frontend (compile <file.tono> [--module <name>] | compile-dir \
     <root> | check <file.tono> | fmt <file.tono> | version)"

  (* Render diagnostics one per line, source order. Shared by the [check]
     command's report. *)
  let render_diags (diags : Diagnostic.t list) : string =
    String.concat "" (List.map (fun d -> Diagnostic.to_string d ^ "\n") diags)

  let has_error (diags : Diagnostic.t list) : bool =
    List.exists (fun (d : Diagnostic.t) -> d.severity = Diagnostic.Error) diags

  (* Pull an optional [--module <name>] out of the compile arguments; the first
     remaining bare argument is the source path. *)
  let rec parse_compile path modname = function
    | [] -> (path, modname)
    | "--module" :: name :: rest -> parse_compile path (Some name) rest
    | arg :: rest ->
        parse_compile (if path = None then Some arg else path) modname rest

  let module_name_of path = function
    | Some name -> name
    | None -> Filename.remove_extension (Filename.basename path)

  (* The qualified module name for a file path relative to the project root: drop
     the [.tono] extension and turn path separators into dots
     (payments/charge.tono -> payments.charge). *)
  let module_name_of_rel (rel : string) : string =
    String.map
      (fun c -> if c = '/' || c = '\\' then '.' else c)
      (Filename.remove_extension rel)

  (* Compile every [.tono] file under [root] into one versioned model. [list_files]
     yields the project's file paths relative to the root; each is read through
     [read_file] rooted at [root]. Paths are sorted so module order is stable. *)
  let compile_dir ~list_files ~read_file root : outcome =
    match list_files root with
    | exception Sys_error msg -> { code = 1; out = ""; err = msg ^ "\n" }
    | rels -> (
        let tono =
          List.sort compare
            (List.filter (fun p -> Filename.check_suffix p ".tono") rels)
        in
        match tono with
        | [] ->
            { code = 1; out = ""; err = "no .tono files under " ^ root ^ "\n" }
        | tono -> (
            match
              List.map
                (fun rel ->
                  (module_name_of_rel rel, read_file (Filename.concat root rel)))
                tono
            with
            | exception Sys_error msg ->
                { code = 1; out = ""; err = msg ^ "\n" }
            | files -> (
                match compile_project_to_json files with
                | Ok json -> { code = 0; out = json ^ "\n"; err = "" }
                | Error msg -> { code = 1; out = ""; err = msg ^ "\n" })))

  let run ?(list_files = fun _ -> []) ~(read_file : string -> string)
      (argv : string array) : outcome =
    match Array.to_list argv with
    | _ :: "compile" :: rest -> (
        match parse_compile None None rest with
        | None, _ -> { code = 2; out = ""; err = usage ^ "\n" }
        | Some path, modname -> (
            match read_file path with
            | exception Sys_error msg ->
                { code = 1; out = ""; err = msg ^ "\n" }
            | src -> (
                let module_name = module_name_of path modname in
                match compile_to_json ~module_name src with
                | Ok json -> { code = 0; out = json ^ "\n"; err = "" }
                | Error msg -> { code = 1; out = ""; err = msg ^ "\n" })))
    | _ :: "compile-dir" :: root :: _ -> compile_dir ~list_files ~read_file root
    | [ _; "compile-dir" ] -> { code = 2; out = ""; err = usage ^ "\n" }
    | _ :: "check" :: path :: _ -> (
        (* Parse, lower, and typecheck, then report every diagnostic (errors and
           warnings alike). Unlike [compile] this emits no IR: the exit code is
           the signal (1 on any error, 0 otherwise) and the report goes to
           stderr so a clean run stays silent. *)
        match read_file path with
        | exception Sys_error msg -> { code = 1; out = ""; err = msg ^ "\n" }
        | src ->
            let _m, diags = compile src in
            let report = render_diags diags in
            if has_error diags then { code = 1; out = ""; err = report }
            else { code = 0; out = ""; err = report })
    | [ _; "check" ] -> { code = 2; out = ""; err = usage ^ "\n" }
    | _ :: "fmt" :: path :: _ -> (
        match read_file path with
        | exception Sys_error msg -> { code = 1; out = ""; err = msg ^ "\n" }
        | src -> (
            match format_source src with
            | Ok formatted -> { code = 0; out = formatted; err = "" }
            | Error msg -> { code = 1; out = ""; err = msg ^ "\n" }))
    | [ _; "fmt" ] -> { code = 2; out = ""; err = usage ^ "\n" }
    | [ _ ] | _ :: "version" :: _ ->
        { code = 0; out = "tono " ^ version ^ "\n"; err = "" }
    | _ -> { code = 2; out = ""; err = usage ^ "\n" }
end
