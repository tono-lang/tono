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

(* The frontend pipeline: lex and parse source text, then lower it to an IR
   module. [module_name] names the resulting module. Diagnostics are returned as
   the lex/parse diagnostics (in source order) followed by the lowering ones. *)
let compile ?(module_name = "") (src : string) : Ir.module_ * Diagnostic.t list
    =
  let file, parse_diags = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name ~diags file in
  (m, parse_diags @ List.rev !diags)
