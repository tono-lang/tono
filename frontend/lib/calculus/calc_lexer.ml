(* Hand-written tokenizer for the calculus, mirroring the .tono lexer's
   byte-by-byte structure (line/column tracking, `//` comments, decoded string
   literals) but over the calculus token set. It never raises: lexical errors
   become diagnostics and scanning continues. '-' is always a [Minus] operator;
   negative literals are folded by the parser, so integer literals are scanned as
   raw digit text (preserving i64). *)

module T = Calc_token

type state = {
  src : string;
  len : int;
  mutable off : int;
  mutable line : int;
  mutable col : int;
  mutable toks : T.t list;
  mutable diags : Diagnostic.t list;
}

let prims =
  [
    "bool";
    "string";
    "bytes";
    "i8";
    "i16";
    "i32";
    "i64";
    "u8";
    "u16";
    "u32";
    "u64";
    "float";
    "timestamp";
    "date";
    "duration";
    "uuid";
  ]

let is_digit c = c >= '0' && c <= '9'

let is_ident_start c =
  c = '_' || (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z')

let is_ident_cont c = is_ident_start c || is_digit c
let is_ws c = c = ' ' || c = '\t' || c = '\r' || c = '\n'
let at_end st = st.off >= st.len
let cur st = st.src.[st.off]
let char_at st i = if i < st.len then Some st.src.[i] else None
let pos st : Span.pos = { line = st.line; col = st.col; offset = st.off }

let bump st =
  if st.src.[st.off] = '\n' then (
    st.line <- st.line + 1;
    st.col <- 1)
  else st.col <- st.col + 1;
  st.off <- st.off + 1

let add_tok st kind ~start =
  st.toks <- { T.kind; span = { Span.start; finish = pos st } } :: st.toks

let add_diag st message ~start =
  st.diags <-
    {
      Diagnostic.span = { Span.start; finish = pos st };
      severity = Diagnostic.Error;
      message;
      code = None;
    }
    :: st.diags

let classify (text : string) : T.kind =
  match text with
  | "fn" -> KwFn
  | "if" -> KwIf
  | "then" -> KwThen
  | "else" -> KwElse
  | "let" -> KwLet
  | "in" -> KwIn
  | "match" -> KwMatch
  | "map" -> KwMap
  | "filter" -> KwFilter
  | "fold" -> KwFold
  | "true" -> KwTrue
  | "false" -> KwFalse
  | "Some" -> KwSome
  | "None" -> KwNone
  | "Unknown" -> KwUnknown
  | _ -> if List.mem text prims then Prim text else Ident text

let scan_ident st start =
  let b = st.off in
  while (not (at_end st)) && is_ident_cont (cur st) do
    bump st
  done;
  add_tok st (classify (String.sub st.src b (st.off - b))) ~start

(* Digits with an optional fractional part. The leading sign, if any, is a
   separate [Minus] token folded by the parser. *)
let scan_number st start =
  let b = st.off in
  while (not (at_end st)) && is_digit (cur st) do
    bump st
  done;
  let is_float =
    (not (at_end st))
    && cur st = '.'
    && match char_at st (st.off + 1) with Some d -> is_digit d | None -> false
  in
  if is_float then (
    bump st;
    while (not (at_end st)) && is_digit (cur st) do
      bump st
    done);
  let text = String.sub st.src b (st.off - b) in
  if is_float then add_tok st (T.Float (float_of_string text)) ~start
  else add_tok st (T.Int text) ~start

(* A double-quoted single-line string with the same escapes as the .tono lexer. *)
let scan_string st start =
  bump st;
  let buf = Buffer.create 16 in
  let stop = ref false in
  while not !stop do
    if at_end st then (
      add_diag st "unterminated string literal" ~start;
      stop := true)
    else
      let c = cur st in
      if c = '"' then (
        bump st;
        stop := true)
      else if c = '\n' then (
        add_diag st "unterminated string literal" ~start;
        stop := true)
      else if c = '\\' then (
        let esc_start = pos st in
        bump st;
        if at_end st then (
          add_diag st "unterminated string literal" ~start;
          stop := true)
        else
          match cur st with
          | '"' ->
              Buffer.add_char buf '"';
              bump st
          | '\\' ->
              Buffer.add_char buf '\\';
              bump st
          | 'n' ->
              Buffer.add_char buf '\n';
              bump st
          | 't' ->
              Buffer.add_char buf '\t';
              bump st
          | 'r' ->
              Buffer.add_char buf '\r';
              bump st
          | other ->
              add_diag st
                (Printf.sprintf "invalid escape '\\%c'" other)
                ~start:esc_start;
              Buffer.add_char buf other;
              bump st)
      else (
        Buffer.add_char buf c;
        bump st)
  done;
  add_tok st (T.Str (Buffer.contents buf)) ~start

let skip_trivia st =
  let stop = ref false in
  while not !stop do
    if at_end st then stop := true
    else
      let c = cur st in
      if is_ws c then bump st
      else if c = '/' && char_at st (st.off + 1) = Some '/' then
        while (not (at_end st)) && cur st <> '\n' do
          bump st
        done
      else stop := true
  done

(* Emit a single-char token, or a two-char token when the next byte matches
   [snd]; [bump]s the consumed bytes. *)
let op2 st start ~next ~two ~one =
  bump st;
  if char_at st st.off = Some next then (
    bump st;
    add_tok st two ~start)
  else add_tok st one ~start

let tokenize (src : string) : T.t list * Diagnostic.t list =
  let st =
    {
      src;
      len = String.length src;
      off = 0;
      line = 1;
      col = 1;
      toks = [];
      diags = [];
    }
  in
  let finished = ref false in
  while not !finished do
    skip_trivia st;
    if at_end st then (
      add_tok st T.Eof ~start:(pos st);
      finished := true)
    else
      let start = pos st in
      match cur st with
      | '(' ->
          bump st;
          add_tok st T.LParen ~start
      | ')' ->
          bump st;
          add_tok st T.RParen ~start
      | '{' ->
          bump st;
          add_tok st T.LBrace ~start
      | '}' ->
          bump st;
          add_tok st T.RBrace ~start
      | '[' ->
          bump st;
          add_tok st T.LBracket ~start
      | ']' ->
          bump st;
          add_tok st T.RBracket ~start
      | ':' ->
          bump st;
          add_tok st T.Colon ~start
      | ',' ->
          bump st;
          add_tok st T.Comma ~start
      | '.' ->
          bump st;
          add_tok st T.Dot ~start
      | '*' ->
          bump st;
          add_tok st T.Star ~start
      | '/' ->
          bump st;
          add_tok st T.Slash ~start
      | '%' ->
          bump st;
          add_tok st T.Percent ~start
      | '+' -> op2 st start ~next:'+' ~two:T.PlusPlus ~one:T.Plus
      | '-' -> op2 st start ~next:'>' ~two:T.Arrow ~one:T.Minus
      | '<' -> op2 st start ~next:'=' ~two:T.LtEq ~one:T.Lt
      | '>' -> op2 st start ~next:'=' ~two:T.GtEq ~one:T.Gt
      | '?' -> op2 st start ~next:'?' ~two:T.QQuestion ~one:T.Question
      | '!' -> op2 st start ~next:'=' ~two:T.BangEq ~one:T.Bang
      | '=' ->
          bump st;
          if char_at st st.off = Some '=' then (
            bump st;
            add_tok st T.EqEq ~start)
          else if char_at st st.off = Some '>' then (
            bump st;
            add_tok st T.FatArrow ~start)
          else add_tok st T.Eq ~start
      | '&' ->
          bump st;
          if char_at st st.off = Some '&' then (
            bump st;
            add_tok st T.AmpAmp ~start)
          else add_diag st "expected '&&'" ~start
      | '|' ->
          bump st;
          if char_at st st.off = Some '|' then (
            bump st;
            add_tok st T.PipePipe ~start)
          else add_diag st "expected '||'" ~start
      | '"' -> scan_string st start
      | c when is_digit c -> scan_number st start
      | c when is_ident_start c -> scan_ident st start
      | c ->
          bump st;
          add_diag st (Printf.sprintf "unexpected character %C" c) ~start
  done;
  (List.rev st.toks, List.rev st.diags)
