(* Lexical tokens produced by the lexer. *)

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
  | Ident of string
  | Prim of string
  | Str of string
  | Foreign of string
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

(* The reserved words with the token each lexes to; the lexer's own table. *)
val keywords : (string * kind) list

(* A human label for a token kind, for diagnostic messages. *)
val describe : kind -> string
