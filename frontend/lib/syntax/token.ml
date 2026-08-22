(* Lexical tokens. Keywords and primitive names are recognized by the lexer;
   HTTP method names (POST, GET, ...) are plain [Ident]s that the parser treats
   specially only in trait-argument position. *)

type kind =
  | KwStruct
  | KwEnum
  | KwUnion
  | KwOp
  | KwMap
  | KwPub
  | KwImport
  | KwAs
  | KwExt
  | KwTest
  | Ident of string (* identifiers and shape/type names, incl. PascalCase *)
  | Prim of string (* a recognized primitive keyword, e.g. "i64" *)
  | Str of string (* decoded string-literal content *)
  | Foreign of string
    (* a foreign spelling, [#(...)]: the bytes between the parentheses,
         verbatim, balanced parentheses included. Never text that crosses as
         data; see [Lexer.scan_foreign]. *)
  | Int of int
  | Float of float
  | At
  | LBrace
  | RBrace
  | LBracket
  | RBracket
  | LParen
  | RParen
  | Colon
  | ColonColon
  | Question
  | Comma
  | Dot
  | Eq
  | Arrow
  | FatArrow
  | Eof

type t = { kind : kind; span : Span.span }

(* The reserved words, each with the token it lexes to. The lexer classifies
   through this table and [Syntax_vocab] enumerates it, so the editor's word
   set is read from the same list the lexer applies. *)
let keywords : (string * kind) list =
  [
    ("struct", KwStruct);
    ("enum", KwEnum);
    ("union", KwUnion);
    ("op", KwOp);
    ("map", KwMap);
    ("pub", KwPub);
    ("import", KwImport);
    ("as", KwAs);
    ("ext", KwExt);
    ("test", KwTest);
  ]

(* A human label for diagnostics, e.g. "expected ':', found '{'". *)
let describe (k : kind) : string =
  match k with
  | KwStruct -> "'struct'"
  | KwEnum -> "'enum'"
  | KwUnion -> "'union'"
  | KwOp -> "'op'"
  | KwMap -> "'map'"
  | KwPub -> "'pub'"
  | KwImport -> "'import'"
  | KwAs -> "'as'"
  | KwExt -> "'ext'"
  | KwTest -> "'test'"
  | Ident s -> Printf.sprintf "identifier '%s'" s
  | Prim s -> Printf.sprintf "type '%s'" s
  | Str _ -> "string literal"
  | Foreign s -> Printf.sprintf "foreign spelling '#(%s)'" s
  | Int n -> Printf.sprintf "integer '%d'" n
  | Float f -> Printf.sprintf "number '%g'" f
  | At -> "'@'"
  | LBrace -> "'{'"
  | RBrace -> "'}'"
  | LBracket -> "'['"
  | RBracket -> "']'"
  | LParen -> "'('"
  | RParen -> "')'"
  | Colon -> "':'"
  | ColonColon -> "'::'"
  | Question -> "'?'"
  | Comma -> "','"
  | Dot -> "'.'"
  | Eq -> "'='"
  | Arrow -> "'->'"
  | FatArrow -> "'=>'"
  | Eof -> "end of file"
