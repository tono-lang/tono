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
module Lower = Lower
module Typecheck = Typecheck

(* The pure calculus: a self-contained, total expression sub-language. Its
   modules carry a [Calc_] prefix because the library namespace is flat. *)
module Calc_ast = Calc_ast
module Calc_token = Calc_token
module Calc_lexer = Calc_lexer
module Calc_parser = Calc_parser

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
