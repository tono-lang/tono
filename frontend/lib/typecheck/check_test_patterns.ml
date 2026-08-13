(* Lowering of expect patterns to the IR pattern language. A pattern is
   ctor-like with three marks ([..] leaves uncited fields free, [any] asserts
   presence, [None] asserts absence); its fields are validated against the
   subject's shape (or the fixed taxonomy/request shapes), and literal leaves
   encode to wire-form equality. *)

module V = Check_test_values

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt

(* A virtual member (the taxonomy and http.request shapes have no user decl):
   built with dummy spans, typed with the surface type grammar so the value
   encoder applies unchanged. *)
let vmember name ty : Ast.member =
  {
    Ast.mname = name;
    mname_span = V.dummy_span;
    mtype = ty;
    mvalue = None;
    mtraits = [];
  }

let str_t = Ast.TPrim ("string", V.dummy_span)
let i32_t = Ast.TPrim ("i32", V.dummy_span)

(* The taxonomy shapes of tono.errors, by category. *)
let taxonomy_members : (string * Ast.member list) list =
  [
    ("api", [ vmember "status" i32_t; vmember "body" str_t ]);
    ("validation", [ vmember "fields" (Ast.TList (str_t, V.dummy_span)) ]);
    ("decode", [ vmember "path" str_t ]);
    ("contract", [ vmember "name" str_t ]);
    ("config", [ vmember "field" str_t ]);
    ("transport", []);
  ]

let taxonomy_categories = List.map fst taxonomy_members

(* http.request, minus [headers], which matches as a subset by name. *)
let request_members : Ast.member list =
  [ vmember "method" str_t; vmember "path" str_t; vmember "body" str_t ]

let pattern_span : Ast.test_pattern -> Span.span = function
  | Ast.TpCtor { tp_span = s; _ }
  | Ast.TpOk s
  | Ast.TpList (_, s)
  | Ast.TpMap { map_span = s; _ }
  | Ast.TpError s ->
      s
  | Ast.TpLit v -> V.value_span v

(* Rebuild the value under a literal-only pattern so the equality leaf reuses
   the typed value encoder; [None] when the pattern contains a mark or a
   nested structural form that cannot be an equality. *)
let rec literal_value : Ast.test_pattern -> Ast.test_value option = function
  | Ast.TpLit v -> Some v
  | Ast.TpList (items, s) ->
      let rec all acc = function
        | [] -> Some (List.rev acc)
        | p :: rest -> (
            match literal_value p with
            | Some v -> all (v :: acc) rest
            | None -> None)
      in
      Option.map (fun vs -> Ast.TvList (vs, s)) (all [] items)
  | Ast.TpMap { entries; map_open = false; map_span } ->
      let rec all acc = function
        | [] -> Some (List.rev acc)
        | (k, Ast.TpfPat p) :: rest -> (
            match literal_value p with
            | Some v -> all ((k, v) :: acc) rest
            | None -> None)
        | _ -> None
      in
      Option.map (fun es -> Ast.TvMap (es, map_span)) (all [] entries)
  | Ast.TpCtor _ | Ast.TpOk _ | Ast.TpMap _ | Ast.TpError _ -> None

(* ── Field patterns against a member list ──────────────────────────────── *)

(* A nested pattern at a member position: an equality on the member's type, or
   a structural pattern when the member is itself a struct. *)
let rec lower_sub_pattern ctx ~refty (mtype : Ast.ty) (p : Ast.test_pattern) :
    Ir.test_pattern =
  match p with
  | Ast.TpError _ -> Ir.P_eq `Null
  | Ast.TpOk s ->
      V.report ctx
        (err Error_codes.test_pattern_invalid s
           "'ok' asserts a whole call outcome, not a field");
      Ir.P_eq `Null
  | Ast.TpCtor c -> (
      match V.base_ty mtype with
      | Ast.TName (n, [], _) -> (
          match V.struct_members ctx n with
          | Some members -> (
              match
                V.resolve_head ctx ~code:Error_codes.test_pattern_invalid
                  c.Ast.tp_head
              with
              | V.Huser head when String.equal head n ->
                  Ir.P_struct
                    {
                      ps_shape = n;
                      ps_open = c.Ast.tp_open;
                      ps_fields =
                        lower_fields ctx ~refty ~shape:n ~members
                          c.Ast.tp_fields;
                    }
              | V.Huser head ->
                  V.report ctx
                    (err Error_codes.test_pattern_invalid c.tp_head.vh_span
                       "expected a '%s' pattern, found '%s'" n head);
                  Ir.P_eq `Null
              | V.Hvirtual _ ->
                  V.report ctx
                    (err Error_codes.test_pattern_invalid c.tp_head.vh_span
                       "language-module shapes cannot nest inside a field \
                        pattern");
                  Ir.P_eq `Null
              | V.Hbad -> Ir.P_eq `Null)
          | None ->
              V.report ctx
                (err Error_codes.test_pattern_invalid c.Ast.tp_span
                   "a structural pattern needs a struct-typed field");
              Ir.P_eq `Null)
      | _ ->
          V.report ctx
            (err Error_codes.test_pattern_invalid c.Ast.tp_span
               "a structural pattern needs a struct-typed field");
          Ir.P_eq `Null)
  | Ast.TpLit _ | Ast.TpList _ | Ast.TpMap _ -> (
      match literal_value p with
      | Some v -> Ir.P_eq (V.encode_value ctx ~refty mtype v)
      | None ->
          V.report ctx
            (err Error_codes.test_pattern_invalid (pattern_span p)
               "an equality pattern must be built from literals alone (no \
                'any', 'None', or '..')");
          Ir.P_eq `Null)

and lower_fields ctx ~refty ~(shape : string) ~(members : Ast.member list)
    (fields : (string * Span.span * Ast.test_pattern_field) list) :
    (string * Ir.field_pattern) list =
  List.filter_map
    (fun (name, name_span, f) ->
      match
        List.find_opt
          (fun (m : Ast.member) -> String.equal m.Ast.mname name)
          members
      with
      | None ->
          V.report ctx
            (err Error_codes.test_pattern_invalid name_span
               "'%s' has no field '%s'" shape name);
          None
      | Some m -> (
          match f with
          | Ast.TpfAny _ -> Some (name, Ir.Fp_present)
          | Ast.TpfAbsent span ->
              if not (V.member_optional m) then
                V.report ctx
                  (err Error_codes.test_pattern_invalid span
                     "field '%s' of '%s' is always present; 'None' asserts the \
                      absence of an optional field"
                     name shape);
              Some (name, Ir.Fp_absent)
          | Ast.TpfPat p ->
              Some (name, Ir.Fp_pat (lower_sub_pattern ctx ~refty m.Ast.mtype p))
          ))
    fields

(* ── Whole-subject structural patterns ─────────────────────────────────── *)

let lower_struct_pattern ctx ~refty ~(shape : string)
    ~(members : Ast.member list) ~(as_error : bool) (c : Ast.test_pattern_ctor)
    : Ir.test_pattern =
  let fields = lower_fields ctx ~refty ~shape ~members c.Ast.tp_fields in
  if as_error then
    Ir.P_error { pe_shape = shape; pe_open = c.Ast.tp_open; pe_fields = fields }
  else
    Ir.P_struct
      { ps_shape = shape; ps_open = c.Ast.tp_open; ps_fields = fields }

(* [errors.<category> { ... }] against the fixed taxonomy shapes. *)
let lower_taxonomy_pattern ctx ~refty ~(category : string)
    (c : Ast.test_pattern_ctor) : Ir.test_pattern option =
  match List.assoc_opt category taxonomy_members with
  | None ->
      V.report ctx
        (err Error_codes.test_pattern_invalid c.Ast.tp_head.vh_span
           "unknown error category 'errors.%s'; the taxonomy is %s" category
           (String.concat ", "
              (List.map (fun c -> "errors." ^ c) taxonomy_categories)));
      None
  | Some members ->
      Some
        (Ir.P_taxonomy
           {
             pt_category = category;
             pt_open = c.Ast.tp_open;
             pt_fields =
               lower_fields ctx ~refty ~shape:("errors." ^ category) ~members
                 c.Ast.tp_fields;
           })

(* ── Request patterns (expect s.requests) ──────────────────────────────── *)

(* The [headers] field of a request pattern: a map subset, one pattern per
   cited header name. *)
let lower_header_entries ctx ~refty
    (entries : ((string * Span.span) * Ast.test_pattern_field) list) :
    (string * Ir.field_pattern) list =
  List.map
    (fun ((key, _), f) ->
      match f with
      | Ast.TpfAny _ -> (key, Ir.Fp_present)
      | Ast.TpfAbsent _ -> (key, Ir.Fp_absent)
      | Ast.TpfPat p -> (key, Ir.Fp_pat (lower_sub_pattern ctx ~refty str_t p)))
    entries

let lower_request_pattern ctx ~refty (p : Ast.test_pattern) :
    Ir.request_pattern option =
  match p with
  | Ast.TpCtor c -> (
      match
        V.resolve_head ctx ~code:Error_codes.test_pattern_invalid c.Ast.tp_head
      with
      | V.Hvirtual (V.Vhttp, "request") ->
          let fields, headers =
            List.partition
              (fun (n, _, _) -> not (String.equal n "headers"))
              c.Ast.tp_fields
          in
          let rp_fields =
            lower_fields ctx ~refty ~shape:"http.request"
              ~members:request_members fields
          in
          let rp_headers =
            match headers with
            | [] -> None
            | (_, span, f) :: _ -> (
                match f with
                | Ast.TpfPat (Ast.TpMap { entries; _ }) ->
                    Some (lower_header_entries ctx ~refty entries)
                | _ ->
                    V.report ctx
                      (err Error_codes.test_pattern_invalid span
                         "'headers' matches as a map subset: headers: { \
                          \"name\": \"value\", .. }");
                    None)
          in
          Some { Ir.rp_open = c.Ast.tp_open; rp_fields; rp_headers }
      | V.Hbad -> None
      | _ ->
          V.report ctx
            (err Error_codes.test_pattern_invalid c.Ast.tp_head.vh_span
               "a '.requests' expect matches http.request patterns");
          None)
  | _ ->
      V.report ctx
        (err Error_codes.test_pattern_invalid (pattern_span p)
           "a '.requests' expect matches a list of http.request patterns");
      None
