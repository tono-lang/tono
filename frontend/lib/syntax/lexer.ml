(* Hand-written tokenizer. It scans the source byte by byte, tracking line/column
   so every token carries a precise span, discards `//` comments, decodes string
   literals, and -- like the parser -- never raises: lexical errors become
   diagnostics and scanning resynchronizes and continues. *)

type state = {
  src : string;
  len : int;
  mutable off : int;
  mutable line : int;
  mutable col : int;
  mutable toks : Token.t list; (* accumulated in reverse *)
  mutable diags : Diagnostic.t list; (* accumulated in reverse *)
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

(* Advance one byte, keeping line/column in sync. The caller guarantees we are
   not at end of input. *)
let bump st =
  if st.src.[st.off] = '\n' then (
    st.line <- st.line + 1;
    st.col <- 1)
  else st.col <- st.col + 1;
  st.off <- st.off + 1

let add_tok st kind ~start =
  st.toks <- { Token.kind; span = { Span.start; finish = pos st } } :: st.toks

let add_diag st severity message ~start =
  st.diags <-
    { Diagnostic.span = { Span.start; finish = pos st }; severity; message }
    :: st.diags

let classify (text : string) : Token.kind =
  match text with
  | "struct" -> KwStruct
  | "enum" -> KwEnum
  | "union" -> KwUnion
  | "op" -> KwOp
  | "map" -> KwMap
  | "pub" -> KwPub
  | "throws" -> KwThrows
  | _ -> if List.mem text prims then Prim text else Ident text

(* A double-quoted single-line string with escapes. Unterminated at end of line
   or end of input, and invalid escapes, are diagnosed; scanning still yields a
   [Str] token with whatever was decoded. *)
let scan_single st start =
  bump st;
  (* opening quote *)
  let buf = Buffer.create 16 in
  let stop = ref false in
  while not !stop do
    if at_end st then (
      add_diag st Diagnostic.Error "unterminated string literal" ~start;
      stop := true)
    else
      let c = cur st in
      if c = '"' then (
        bump st;
        stop := true)
      else if c = '\n' then (
        add_diag st Diagnostic.Error "unterminated string literal" ~start;
        stop := true)
      else if c = '\\' then (
        let esc_start = pos st in
        bump st;
        if at_end st then (
          add_diag st Diagnostic.Error "unterminated string literal" ~start;
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
              add_diag st Diagnostic.Error
                (Printf.sprintf "invalid escape '\\%c'" other)
                ~start:esc_start;
              Buffer.add_char buf other;
              bump st)
      else (
        Buffer.add_char buf c;
        bump st)
  done;
  add_tok st (Token.Str (Buffer.contents buf)) ~start

(* A triple-quoted multi-line string: raw content (no escapes) up to the next
   closing triple quote, or a diagnostic at end of input. *)
let scan_triple st start =
  bump st;
  bump st;
  bump st;
  (* opening triple quote *)
  let buf = Buffer.create 32 in
  let stop = ref false in
  while not !stop do
    if at_end st then (
      add_diag st Diagnostic.Error "unterminated triple-quoted string" ~start;
      stop := true)
    else if
      cur st = '"'
      && char_at st (st.off + 1) = Some '"'
      && char_at st (st.off + 2) = Some '"'
    then (
      bump st;
      bump st;
      bump st;
      stop := true)
    else (
      Buffer.add_char buf (cur st);
      bump st)
  done;
  add_tok st (Token.Str (Buffer.contents buf)) ~start

let scan_string st start =
  if char_at st (st.off + 1) = Some '"' && char_at st (st.off + 2) = Some '"'
  then scan_triple st start
  else scan_single st start

(* A numeric literal: an optional leading '-', digits, and an optional
   fractional '.' digits part (which makes it a float). The caller guarantees the
   cursor is on '-' or a digit. *)
let scan_number st start =
  let b = st.off in
  if cur st = '-' then bump st;
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
    (* '.' *)
    while (not (at_end st)) && is_digit (cur st) do
      bump st
    done);
  let text = String.sub st.src b (st.off - b) in
  if is_float then
    (* [text] is always a syntactically valid float here, so this never fails;
       a huge magnitude becomes infinity rather than raising. *)
    add_tok st (Token.Float (float_of_string text)) ~start
  else
    match int_of_string_opt text with
    | Some n -> add_tok st (Token.Int n) ~start
    | None ->
        add_diag st Diagnostic.Error
          (Printf.sprintf "integer literal '%s' is out of range" text)
          ~start;
        add_tok st (Token.Int 0) ~start

let scan_ident st start =
  let b = st.off in
  while (not (at_end st)) && is_ident_cont (cur st) do
    bump st
  done;
  add_tok st (classify (String.sub st.src b (st.off - b))) ~start

(* Skip whitespace and `//` line comments. *)
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

let tokenize (src : string) : Token.t list * Diagnostic.t list =
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
      add_tok st Token.Eof ~start:(pos st);
      finished := true)
    else
      let start = pos st in
      let c = cur st in
      match c with
      | '{' ->
          bump st;
          add_tok st Token.LBrace ~start
      | '}' ->
          bump st;
          add_tok st Token.RBrace ~start
      | '[' ->
          bump st;
          add_tok st Token.LBracket ~start
      | ']' ->
          bump st;
          add_tok st Token.RBracket ~start
      | '(' ->
          bump st;
          add_tok st Token.LParen ~start
      | ')' ->
          bump st;
          add_tok st Token.RParen ~start
      | ':' ->
          bump st;
          add_tok st Token.Colon ~start
      | '?' ->
          bump st;
          add_tok st Token.Question ~start
      | ',' ->
          bump st;
          add_tok st Token.Comma ~start
      | '.' ->
          bump st;
          add_tok st Token.Dot ~start
      | '=' ->
          bump st;
          add_tok st Token.Eq ~start
      | '-' when char_at st (st.off + 1) = Some '>' ->
          bump st;
          bump st;
          add_tok st Token.Arrow ~start
      | '-'
        when match char_at st (st.off + 1) with
             | Some d -> is_digit d
             | None -> false ->
          scan_number st start
      | '@' ->
          bump st;
          add_tok st Token.At ~start
      | '"' -> scan_string st start
      | c when is_digit c -> scan_number st start
      | c when is_ident_start c -> scan_ident st start
      | _ ->
          bump st;
          add_diag st Diagnostic.Error
            (Printf.sprintf "unexpected character %C" c)
            ~start
  done;
  (List.rev st.toks, List.rev st.diags)
