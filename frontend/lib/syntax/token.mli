(* Lexical tokens produced by the lexer. *)

type kind =
  | KwStruct
  | KwEnum
  | KwUnion
  | KwOp
  | KwMap
  | KwPub
  | Ident of string
  | Prim of string
  | Str of string
  | Int of int
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
  | Eof

type t = { kind : kind; span : Span.span }

(* A human label for a token kind, for diagnostic messages. *)
val describe : kind -> string
