(* JSON codecs for the declared-test surface (constructions, stubs, calls, and
   expect patterns). [Ir_json] folds these into the module encoding; the Rust
   backend mirrors this contract. Values and inputs are opaque wire-form JSON
   carried verbatim; only the pattern and answer structures are typed here. *)

let ( let* ) = Result.bind
let err fmt = Printf.ksprintf (fun s -> Error s) fmt
let map_result = Ir_json_base.map_result
let as_assoc = Ir_json_base.as_assoc
let as_list = Ir_json_base.as_list
let as_string = Ir_json_base.as_string
let as_bool = Ir_json_base.as_bool
let as_int = Ir_json_base.as_int

(* ── Encoding ──────────────────────────────────────────────────────────── *)

let encode_dep = function Ir.Dep_http -> "http" | Ir.Dep_impl -> "impl"

let encode_construction (c : Ir.test_construction) : Ir.json =
  `Assoc
    [
      ("binding", `String c.tc_binding);
      ("entry", `String c.tc_entry);
      ("values", `Assoc c.tc_values);
    ]

let encode_answer (a : Ir.stub_answer) : Ir.json =
  match a with
  | Ir.Answer_http { ans_status; ans_headers; ans_body } ->
      `Assoc
        [
          ("status", `Int ans_status);
          ( "headers",
            `Assoc (List.map (fun (k, v) -> (k, `String v)) ans_headers) );
          ("body", `String ans_body);
        ]
  | Ir.Answer_value v -> `Assoc [ ("value", v) ]
  | Ir.Answer_error { ans_shape; ans_data } ->
      `Assoc
        [
          ("error", `Assoc [ ("shape", `String ans_shape); ("data", ans_data) ]);
        ]
  | Ir.Answer_contract -> `Assoc [ ("contract", `Assoc []) ]

let encode_stub (s : Ir.test_stub) : Ir.json =
  `Assoc
    ((match s.ts_binding with
       | Some b -> [ ("binding", `String b) ]
       | None -> [])
    @ [
        ("client", `String s.ts_client);
        ("op", `String s.ts_op);
        ("dep", `String (encode_dep s.ts_dep));
        ("answers", `List (List.map encode_answer s.ts_answers));
      ])

let encode_extern_target (t : Ir.extern_stub_target) : Ir.json =
  match t with
  | Ir.Ext_free { lib; fn } ->
      `Assoc [ ("lib", `String lib); ("fn", `String fn) ]
  | Ir.Ext_method { lib; ty; meth } ->
      `Assoc
        [ ("lib", `String lib); ("type", `String ty); ("method", `String meth) ]

let encode_extern_stub (s : Ir.extern_stub) : Ir.json =
  `Assoc
    ((match s.es_binding with
       | Some b -> [ ("binding", `String b) ]
       | None -> [])
    @ [
        ("target", encode_extern_target s.es_target);
        ("answers", `List (List.map encode_answer s.es_answers));
      ])

let encode_call (c : Ir.test_call) : Ir.json =
  `Assoc
    ([
       ("binding", `String c.call_binding);
       ("client", `String c.call_client);
       ("op", `String c.call_op);
     ]
    @ match c.call_input with None -> [] | Some v -> [ ("input", v) ])

let rec encode_pattern (p : Ir.test_pattern) : Ir.json =
  let structural shape open_ fields extra_key =
    `Assoc
      [
        (extra_key, `String shape);
        ("open", `Bool open_);
        ( "fields",
          `Assoc (List.map (fun (k, fp) -> (k, encode_field_pattern fp)) fields)
        );
      ]
  in
  match p with
  | Ir.P_eq v -> `Assoc [ ("eq", v) ]
  | Ir.P_struct { ps_shape; ps_open; ps_fields } ->
      `Assoc [ ("struct", structural ps_shape ps_open ps_fields "shape") ]
  | Ir.P_error { pe_shape; pe_open; pe_fields } ->
      `Assoc [ ("error", structural pe_shape pe_open pe_fields "shape") ]
  | Ir.P_taxonomy { pt_category; pt_open; pt_fields } ->
      `Assoc
        [ ("taxonomy", structural pt_category pt_open pt_fields "category") ]
  | Ir.P_ok -> `Assoc [ ("ok", `Assoc []) ]

and encode_field_pattern (fp : Ir.field_pattern) : Ir.json =
  match fp with
  | Ir.Fp_pat p -> encode_pattern p
  | Ir.Fp_absent -> `Assoc [ ("absent", `Assoc []) ]
  | Ir.Fp_present -> `Assoc [ ("present", `Assoc []) ]

let encode_request_pattern (rp : Ir.request_pattern) : Ir.json =
  let fields =
    List.map (fun (k, fp) -> (k, encode_field_pattern fp)) rp.rp_fields
    @
    match rp.rp_headers with
    | None -> []
    | Some hs ->
        [
          ( "headers",
            `Assoc (List.map (fun (k, fp) -> (k, encode_field_pattern fp)) hs)
          );
        ]
  in
  `Assoc [ ("open", `Bool rp.rp_open); ("fields", `Assoc fields) ]

let encode_expect (e : Ir.test_expect) : Ir.json =
  match e with
  | Ir.Expect_outcome { ex_subject; ex_pattern } ->
      `Assoc
        [
          ("subject", `String ex_subject); ("pattern", encode_pattern ex_pattern);
        ]
  | Ir.Expect_requests { ex_subject; ex_requests } ->
      `Assoc
        [
          ("subject", `String ex_subject);
          ("requests", `List (List.map encode_request_pattern ex_requests));
        ]

let encode_test (t : Ir.test_decl) : Ir.json =
  `Assoc
    [
      ("name", `String t.t_name);
      ("constructions", `List (List.map encode_construction t.t_constructions));
      ("stubs", `List (List.map encode_stub t.t_stubs));
      ("extern_stubs", `List (List.map encode_extern_stub t.t_extern_stubs));
      ("calls", `List (List.map encode_call t.t_calls));
      ("expects", `List (List.map encode_expect t.t_expects));
    ]

(* ── Decoding ──────────────────────────────────────────────────────────── *)

let decode_dep j =
  let* s = as_string j in
  match s with
  | "http" -> Ok Ir.Dep_http
  | "impl" -> Ok Ir.Dep_impl
  | other -> err "unknown stub dependency %S" other

let decode_construction j =
  let* kvs = as_assoc j in
  let get k = List.assoc_opt k kvs in
  let* binding =
    match get "binding" with
    | Some v -> as_string v
    | None -> err "construction is missing binding"
  in
  let* entry =
    match get "entry" with
    | Some v -> as_string v
    | None -> err "construction is missing entry"
  in
  let* values =
    match get "values" with None -> Ok [] | Some v -> as_assoc v
  in
  Ok
    ({ tc_binding = binding; tc_entry = entry; tc_values = values }
      : Ir.test_construction)

let decode_answer j =
  let* kvs = as_assoc j in
  match List.assoc_opt "status" kvs with
  | Some sv ->
      let* status = as_int sv in
      let* headers =
        match List.assoc_opt "headers" kvs with
        | None -> Ok []
        | Some v ->
            let* hs = as_assoc v in
            map_result
              (fun (k, hv) ->
                let* s = as_string hv in
                Ok (k, s))
              hs
      in
      let* body =
        match List.assoc_opt "body" kvs with
        | None -> Ok ""
        | Some v -> as_string v
      in
      Ok
        (Ir.Answer_http
           { ans_status = status; ans_headers = headers; ans_body = body })
  | None -> (
      match kvs with
      | [ ("value", v) ] -> Ok (Ir.Answer_value v)
      | [ ("error", ev) ] ->
          let* ekvs = as_assoc ev in
          let* shape =
            match List.assoc_opt "shape" ekvs with
            | Some v -> as_string v
            | None -> err "error answer is missing shape"
          in
          let data =
            match List.assoc_opt "data" ekvs with
            | Some v -> v
            | None -> `Assoc []
          in
          Ok (Ir.Answer_error { ans_shape = shape; ans_data = data })
      | [ ("contract", _) ] -> Ok Ir.Answer_contract
      | _ ->
          err "stub answer must be an http response or a value/error/contract")

let decode_stub j =
  let* kvs = as_assoc j in
  let get k = List.assoc_opt k kvs in
  let* binding =
    match get "binding" with
    | None -> Ok None
    | Some v ->
        let* s = as_string v in
        Ok (Some s)
  in
  let req k =
    match get k with
    | Some v -> as_string v
    | None -> err "stub is missing %s" k
  in
  let* client = req "client" in
  let* op = req "op" in
  let* dep =
    match get "dep" with
    | Some v -> decode_dep v
    | None -> err "stub is missing dep"
  in
  let* answers =
    match get "answers" with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_answer xs
  in
  Ok
    ({
       ts_binding = binding;
       ts_client = client;
       ts_op = op;
       ts_dep = dep;
       ts_answers = answers;
     }
      : Ir.test_stub)

let decode_extern_target j =
  let* kvs = as_assoc j in
  let get k = List.assoc_opt k kvs in
  let req k =
    match get k with
    | Some v -> as_string v
    | None -> err "extern stub target is missing %s" k
  in
  let* lib = req "lib" in
  match get "method" with
  | Some mv ->
      let* meth = as_string mv in
      let* ty = req "type" in
      Ok (Ir.Ext_method { lib; ty; meth })
  | None ->
      let* fn = req "fn" in
      Ok (Ir.Ext_free { lib; fn })

let decode_extern_stub j =
  let* kvs = as_assoc j in
  let get k = List.assoc_opt k kvs in
  let* binding =
    match get "binding" with
    | None -> Ok None
    | Some v ->
        let* s = as_string v in
        Ok (Some s)
  in
  let* target =
    match get "target" with
    | Some v -> decode_extern_target v
    | None -> err "extern stub is missing target"
  in
  let* answers =
    match get "answers" with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode_answer xs
  in
  Ok
    ({ es_binding = binding; es_target = target; es_answers = answers }
      : Ir.extern_stub)

let decode_call j =
  let* kvs = as_assoc j in
  let get k = List.assoc_opt k kvs in
  let req k =
    match get k with
    | Some v -> as_string v
    | None -> err "call is missing %s" k
  in
  let* binding = req "binding" in
  let* client = req "client" in
  let* op = req "op" in
  let input =
    match get "input" with Some `Null | None -> None | Some v -> Some v
  in
  Ok
    ({
       call_binding = binding;
       call_client = client;
       call_op = op;
       call_input = input;
     }
      : Ir.test_call)

let rec decode_pattern j =
  let* kvs = as_assoc j in
  let structural v key =
    let* skvs = as_assoc v in
    let* tag =
      match List.assoc_opt key skvs with
      | Some t -> as_string t
      | None -> err "structural pattern is missing %s" key
    in
    let* open_ =
      match List.assoc_opt "open" skvs with
      | None -> Ok false
      | Some b -> as_bool b
    in
    let* fields =
      match List.assoc_opt "fields" skvs with
      | None -> Ok []
      | Some f ->
          let* fkvs = as_assoc f in
          map_result
            (fun (k, fv) ->
              let* fp = decode_field_pattern fv in
              Ok (k, fp))
            fkvs
    in
    Ok (tag, open_, fields)
  in
  match kvs with
  | [ ("eq", v) ] -> Ok (Ir.P_eq v)
  | [ ("struct", v) ] ->
      let* shape, open_, fields = structural v "shape" in
      Ok (Ir.P_struct { ps_shape = shape; ps_open = open_; ps_fields = fields })
  | [ ("error", v) ] ->
      let* shape, open_, fields = structural v "shape" in
      Ok (Ir.P_error { pe_shape = shape; pe_open = open_; pe_fields = fields })
  | [ ("taxonomy", v) ] ->
      let* category, open_, fields = structural v "category" in
      Ok
        (Ir.P_taxonomy
           { pt_category = category; pt_open = open_; pt_fields = fields })
  | [ ("ok", _) ] -> Ok Ir.P_ok
  | _ -> err "pattern must be a single eq/struct/error/taxonomy/ok key"

and decode_field_pattern j =
  let* kvs = as_assoc j in
  match kvs with
  | [ ("absent", _) ] -> Ok Ir.Fp_absent
  | [ ("present", _) ] -> Ok Ir.Fp_present
  | _ -> (
      match decode_pattern j with
      | Ok p -> Ok (Ir.Fp_pat p)
      | Error e -> Error e)

let decode_request_pattern j =
  let* kvs = as_assoc j in
  let* open_ =
    match List.assoc_opt "open" kvs with
    | None -> Ok false
    | Some b -> as_bool b
  in
  let* all_fields =
    match List.assoc_opt "fields" kvs with
    | None -> Ok []
    | Some f -> as_assoc f
  in
  let plain, headers =
    List.partition (fun (k, _) -> not (String.equal k "headers")) all_fields
  in
  let* rp_fields =
    map_result
      (fun (k, fv) ->
        let* fp = decode_field_pattern fv in
        Ok (k, fp))
      plain
  in
  let* rp_headers =
    match headers with
    | [] -> Ok None
    | (_, hv) :: _ ->
        let* hkvs = as_assoc hv in
        let* hs =
          map_result
            (fun (k, fv) ->
              let* fp = decode_field_pattern fv in
              Ok (k, fp))
            hkvs
        in
        Ok (Some hs)
  in
  Ok ({ rp_open = open_; rp_fields; rp_headers } : Ir.request_pattern)

let decode_expect j =
  let* kvs = as_assoc j in
  let* subject =
    match List.assoc_opt "subject" kvs with
    | Some v -> as_string v
    | None -> err "expect is missing subject"
  in
  match List.assoc_opt "requests" kvs with
  | Some v ->
      let* xs = as_list v in
      let* requests = map_result decode_request_pattern xs in
      Ok (Ir.Expect_requests { ex_subject = subject; ex_requests = requests })
  | None -> (
      match List.assoc_opt "pattern" kvs with
      | Some v ->
          let* pattern = decode_pattern v in
          Ok (Ir.Expect_outcome { ex_subject = subject; ex_pattern = pattern })
      | None -> err "expect carries neither pattern nor requests")

let decode_test j =
  let* kvs = as_assoc j in
  let get k = List.assoc_opt k kvs in
  let* name =
    match get "name" with
    | Some v -> as_string v
    | None -> err "test is missing name"
  in
  let list k decode =
    match get k with
    | None -> Ok []
    | Some v ->
        let* xs = as_list v in
        map_result decode xs
  in
  let* constructions = list "constructions" decode_construction in
  let* stubs = list "stubs" decode_stub in
  let* extern_stubs = list "extern_stubs" decode_extern_stub in
  let* calls = list "calls" decode_call in
  let* expects = list "expects" decode_expect in
  Ok
    ({
       t_name = name;
       t_constructions = constructions;
       t_stubs = stubs;
       t_extern_stubs = extern_stubs;
       t_calls = calls;
       t_expects = expects;
     }
      : Ir.test_decl)
