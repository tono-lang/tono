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
    let model =
      { Ir.tono_ir_version = Ir_json.current_ir_version; modules = [ m ] }
    in
    Ok (Ir_json.to_canonical_string (Ir_json.encode_model model))

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
    "usage: tono-frontend (compile <file.tono> [--module <name>] | fmt \
     <file.tono> | version)"

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

  let run ~(read_file : string -> string) (argv : string array) : outcome =
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
