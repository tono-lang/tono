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
  | KwThrows
  | Ident of string (* identifiers and shape/type names, incl. PascalCase *)
  | Prim of string (* a recognized primitive keyword, e.g. "i64" *)
  | Str of string (* decoded string-literal content *)
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
  | Question
  | Comma
  | Dot
  | Eq
  | Arrow
  | Eof

type t = { kind : kind; span : Span.span }

(* A human label for diagnostics, e.g. "expected ':', found '{'". *)
let describe (k : kind) : string =
  match k with
  | KwStruct -> "'struct'"
  | KwEnum -> "'enum'"
  | KwUnion -> "'union'"
  | KwOp -> "'op'"
  | KwMap -> "'map'"
  | KwPub -> "'pub'"
  | KwThrows -> "'throws'"
  | Ident s -> Printf.sprintf "identifier '%s'" s
  | Prim s -> Printf.sprintf "type '%s'" s
  | Str _ -> "string literal"
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
  | Question -> "'?'"
  | Comma -> "','"
  | Dot -> "'.'"
  | Eq -> "'='"
  | Arrow -> "'->'"
  | Eof -> "end of file"
