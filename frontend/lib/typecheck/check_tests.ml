(* Declared-test validation and lowering. A test is a script of named bindings
   (construction, stub, call) plus expects, every reference pointing backwards;
   this pass resolves each reference against the bindings declared so far,
   types every value against the declaration it fills, and lowers the block to
   the IR test encoding in one walk (the wire encoding is type-driven, so
   lowering cannot happen before the checks). *)

module V = Check_test_values
module Pt = Check_test_patterns

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt

(* What a binding names: the constructed entry, a stub, or a call outcome. *)
type bkind =
  | Bentry of Ast.decl
  | Bstub of { dep : Ir.test_dep }
  | Bcall of { op : Ast.decl }

(* ── Operation helpers ─────────────────────────────────────────────────── *)

let entry_ops (d : Ast.decl) : Ast.decl list =
  match d.Ast.dkind with Ast.DStruct { ops; _ } -> ops | _ -> []

let op_input (op : Ast.decl) : Ast.ty option =
  match op.Ast.dkind with Ast.DOp { input; _ } -> input | _ -> None

let op_output (op : Ast.decl) : Ast.ty option =
  match op.Ast.dkind with Ast.DOp { output; _ } -> output | _ -> None

let output_shape_name (op : Ast.decl) : string option =
  match op_output op with
  | Some ty -> (
      match V.base_ty ty with Ast.TName (n, [], _) -> Some n | _ -> None)
  | None -> None

(* The shapes an operation names in its @errors traits. *)
let declared_error_names (op : Ast.decl) : string list =
  List.concat_map
    (fun (tr : Ast.trait) ->
      if String.equal tr.Ast.tname "errors" then
        List.filter_map
          (function Ast.AName n -> Some n | _ -> None)
          tr.Ast.targs
      else [])
    op.Ast.dtraits

(* Whether an "ext impl" covers [op] of [entry]: the impl names the bare
   operation or the qualified "entry.op" (mirrors Check_impl's resolution). *)
let impl_covers (decls : Ast.decl list) ~(entry : Ast.decl) ~(op : Ast.decl) :
    bool =
  let names = [ op.Ast.dname; entry.Ast.dname ^ "." ^ op.Ast.dname ] in
  List.exists
    (fun (d : Ast.decl) ->
      match d.Ast.dkind with
      | Ast.DExt { ekind = Ast.EImpl; _ } -> List.mem d.Ast.dname names
      | _ -> false)
    decls

(* A shape that is both the output and a declared error of one operation makes
   its ctor unreadable in a stub value or an expect pattern. *)
let ambiguity ctx (op : Ast.decl) (name : string) span : bool =
  let ambiguous =
    output_shape_name op = Some name && List.mem name (declared_error_names op)
  in
  if ambiguous then
    V.report ctx
      (err Error_codes.test_shape_ambiguous span
         "'%s' is both the output and a declared error of '%s'; the pattern \
          cannot tell success from failure"
         name op.Ast.dname);
  ambiguous

(* ── Dataflow references ───────────────────────────────────────────────── *)

(* Type [saved.id] against the bindings in scope: the base must be a call
   binding, and the path descends through the output shape's members. *)
let make_refty ctx (scope : (string * bkind) list ref) : V.ref_typer =
  let rec descend ty segs span =
    match segs with
    | [] -> Some ty
    | s :: rest -> (
        match V.base_ty ty with
        | Ast.TName (n, [], _) -> (
            match V.struct_members ctx n with
            | Some members -> (
                match
                  List.find_opt
                    (fun (m : Ast.member) -> String.equal m.Ast.mname s)
                    members
                with
                | Some m -> descend m.Ast.mtype rest span
                | None ->
                    V.report ctx
                      (err Error_codes.test_value_invalid span
                         "'%s' has no field '%s'" n s);
                    None)
            | None ->
                V.report ctx
                  (err Error_codes.test_value_invalid span
                     "'%s' is not a struct; '.%s' does not resolve" n s);
                None)
        | _ ->
            V.report ctx
              (err Error_codes.test_value_invalid span
                 "'.%s' does not resolve on this type" s);
            None)
  in
  fun ~base ~path span ->
    match List.assoc_opt base !scope with
    | None ->
        V.report ctx
          (err Error_codes.test_binding_unknown span
             "unknown binding '%s'; a test references bindings declared above"
             base);
        None
    | Some (Bcall { op }) -> (
        match op_output op with
        | Some ty -> descend ty path span
        | None ->
            V.report ctx
              (err Error_codes.test_value_invalid span
                 "operation '%s' has no output to read from" op.Ast.dname);
            None)
    | Some _ ->
        V.report ctx
          (err Error_codes.test_value_invalid span
             "dataflow reads the output of a call binding");
        None

(* ── Stub answers ──────────────────────────────────────────────────────── *)

let http_response_answer ctx ~refty (v : Ast.test_value) : Ir.stub_answer option
    =
  match v with
  | Ast.TvCtor c -> (
      match
        V.resolve_head ctx ~code:Error_codes.test_stub_value_invalid c.tc_head
      with
      | V.Hvirtual (V.Vhttp, "response") -> (
          let status = ref None in
          let headers = ref [] in
          let body = ref "" in
          List.iter
            (fun (name, name_span, fv) ->
              match (name, fv) with
              | "status", Ast.TvInt (n, _) -> status := Some n
              | "status", _ ->
                  V.report ctx
                    (err Error_codes.test_stub_value_invalid name_span
                       "http.response status is an integer")
              | "headers", Ast.TvMap (entries, _) ->
                  headers :=
                    List.filter_map
                      (fun ((k, _), hv) ->
                        match hv with
                        | Ast.TvStr (s, _) -> Some (k, s)
                        | _ ->
                            V.report ctx
                              (err Error_codes.test_stub_value_invalid
                                 (V.value_span hv) "header values are strings");
                            None)
                      entries
              | "headers", _ ->
                  V.report ctx
                    (err Error_codes.test_stub_value_invalid name_span
                       "http.response headers is a map of strings")
              | "body", Ast.TvStr (s, _) -> body := s
              | "body", _ ->
                  V.report ctx
                    (err Error_codes.test_stub_value_invalid name_span
                       "http.response body is a string")
              | _ ->
                  V.report ctx
                    (err Error_codes.test_stub_value_invalid name_span
                       "http.response has no field '%s'" name))
            c.tc_fields;
          ignore refty;
          match !status with
          | Some s ->
              Some
                (Ir.Answer_http
                   { ans_status = s; ans_headers = !headers; ans_body = !body })
          | None ->
              V.report ctx
                (err Error_codes.test_stub_value_invalid c.tc_span
                   "http.response requires a status");
              None)
      | V.Hbad -> None
      | _ ->
          V.report ctx
            (err Error_codes.test_stub_value_invalid c.tc_head.vh_span
               "an http stub answers with http.response values");
          None)
  | _ ->
      V.report ctx
        (err Error_codes.test_stub_value_invalid (V.value_span v)
           "an http stub answers with http.response values (or a list of them)");
      None

let impl_answer ctx ~refty (op : Ast.decl) (v : Ast.test_value) :
    Ir.stub_answer option =
  match v with
  | Ast.TvCtor c -> (
      match
        V.resolve_head ctx ~code:Error_codes.test_stub_value_invalid c.tc_head
      with
      | V.Huser n ->
          if ambiguity ctx op n c.tc_head.vh_span then None
          else if output_shape_name op = Some n then
            match op_output op with
            | Some ty -> Some (Ir.Answer_value (V.encode_value ctx ~refty ty v))
            | None -> None
          else if List.mem n (declared_error_names op) then
            let data =
              V.encode_value ctx ~refty (Ast.TName (n, [], c.tc_head.vh_span)) v
            in
            Some (Ir.Answer_error { ans_shape = n; ans_data = data })
          else (
            V.report ctx
              (err Error_codes.test_stub_value_invalid c.tc_head.vh_span
                 "'%s' is neither the output nor a declared error of operation \
                  '%s'"
                 n op.Ast.dname);
            None)
      | V.Hvirtual (V.Verrors, "contract") ->
          (match c.tc_fields with
          | [] -> ()
          | (_, span, _) :: _ ->
              V.report ctx
                (err Error_codes.test_stub_value_invalid span
                   "errors.contract carries no data in a stub answer"));
          Some Ir.Answer_contract
      | V.Hbad -> None
      | _ ->
          V.report ctx
            (err Error_codes.test_stub_value_invalid c.tc_head.vh_span
               "an impl stub answers with the operation's output, a declared \
                error, or errors.contract");
          None)
  | _ ->
      V.report ctx
        (err Error_codes.test_stub_value_invalid (V.value_span v)
           "an impl stub answers with the operation's output, a declared \
            error, or errors.contract (or a list of them)");
      None

let stub_answers ctx ~refty (dep : Ir.test_dep) (op : Ast.decl)
    (value : Ast.test_value) : Ir.stub_answer list =
  let one v =
    match dep with
    | Ir.Dep_http -> http_response_answer ctx ~refty v
    | Ir.Dep_impl -> impl_answer ctx ~refty op v
  in
  match value with
  | Ast.TvList ([], span) ->
      V.report ctx
        (err Error_codes.test_stub_value_invalid span
           "a stub sequence needs at least one answer");
      []
  | Ast.TvList (items, _) -> List.filter_map one items
  | v -> Option.to_list (one v)

(* ── Expect patterns per subject ───────────────────────────────────────── *)

let construction_categories = [ "config"; "validation"; "contract" ]

let construction_pattern ctx ~refty (p : Ast.test_pattern) :
    Ir.test_pattern option =
  match p with
  | Ast.TpOk _ -> Some Ir.P_ok
  | Ast.TpCtor c -> (
      match
        V.resolve_head ctx ~code:Error_codes.test_pattern_invalid c.tp_head
      with
      | V.Hvirtual (V.Verrors, cat) when List.mem cat construction_categories ->
          Pt.lower_taxonomy_pattern ctx ~refty ~category:cat c
      | V.Hvirtual (V.Verrors, cat) when List.mem cat Pt.taxonomy_categories ->
          V.report ctx
            (err Error_codes.test_pattern_invalid c.tp_head.vh_span
               "a construction fails with errors.config, errors.validation, or \
                errors.contract; 'errors.%s' is a call outcome"
               cat);
          None
      | V.Hvirtual (V.Verrors, cat) ->
          Pt.lower_taxonomy_pattern ctx ~refty ~category:cat c
      | V.Hbad -> None
      | _ ->
          V.report ctx
            (err Error_codes.test_pattern_invalid c.tp_head.vh_span
               "a construction expect asserts 'ok' or an errors.* outcome");
          None)
  | _ ->
      V.report ctx
        (err Error_codes.test_pattern_invalid (Pt.pattern_span p)
           "a construction expect asserts 'ok' or an errors.* outcome");
      None

let call_pattern ctx ~refty (op : Ast.decl) (p : Ast.test_pattern) :
    Ir.test_pattern option =
  match p with
  | Ast.TpOk span ->
      (match op_output op with
      | Some _ ->
          V.report ctx
            (err Error_codes.test_pattern_invalid span
               "'ok' asserts an operation without output; '%s' returns a \
                value, match it"
               op.Ast.dname)
      | None -> ());
      Some Ir.P_ok
  | Ast.TpCtor c -> (
      match
        V.resolve_head ctx ~code:Error_codes.test_pattern_invalid c.tp_head
      with
      | V.Hvirtual (V.Verrors, cat) ->
          Pt.lower_taxonomy_pattern ctx ~refty ~category:cat c
      | V.Hvirtual (V.Vhttp, _) ->
          V.report ctx
            (err Error_codes.test_pattern_invalid c.tp_head.vh_span
               "http.* shapes belong to stub values and '.requests' patterns");
          None
      | V.Huser n ->
          if ambiguity ctx op n c.tp_head.vh_span then None
          else if output_shape_name op = Some n then
            match V.struct_members ctx n with
            | Some members ->
                Some
                  (Pt.lower_struct_pattern ctx ~refty ~shape:n ~members
                     ~as_error:false c)
            | None -> None
          else if List.mem n (declared_error_names op) then
            match V.struct_members ctx n with
            | Some members ->
                Some
                  (Pt.lower_struct_pattern ctx ~refty ~shape:n ~members
                     ~as_error:true c)
            | None -> None
          else (
            V.report ctx
              (err Error_codes.test_pattern_invalid c.tp_head.vh_span
                 "'%s' is neither the output nor a declared error of operation \
                  '%s'"
                 n op.Ast.dname);
            None)
      | V.Hbad -> None)
  | _ -> (
      match op_output op with
      | Some ty -> Some (Pt.lower_sub_pattern ctx ~refty ty p)
      | None ->
          V.report ctx
            (err Error_codes.test_pattern_invalid (Pt.pattern_span p)
               "operation '%s' has no output to compare; assert 'ok'"
               op.Ast.dname);
          None)

let requests_expect ctx ~refty (dep : Ir.test_dep) subject
    (p : Ast.test_pattern) : Ir.test_expect option =
  match dep with
  | Ir.Dep_impl ->
      V.report ctx
        (err Error_codes.test_pattern_invalid (Pt.pattern_span p)
           "'.requests' patterns cover http stubs; asserting the typed inputs \
            of an impl stub is not supported");
      None
  | Ir.Dep_http -> (
      match p with
      | Ast.TpList (items, _) ->
          Some
            (Ir.Expect_requests
               {
                 ex_subject = subject;
                 ex_requests =
                   List.filter_map (Pt.lower_request_pattern ctx ~refty) items;
               })
      | _ ->
          V.report ctx
            (err Error_codes.test_pattern_invalid (Pt.pattern_span p)
               "'.requests' asserts a list of http.request patterns");
          None)

(* ── The per-test walk ─────────────────────────────────────────────────── *)

let add_binding ctx (scope : (string * bkind) list ref) name span kind =
  if List.mem_assoc name !scope then
    V.report ctx
      (err Error_codes.test_binding_duplicate span
         "binding '%s' is already declared in this test" name)
  else scope := (name, kind) :: !scope

let check_test ctx (d : Ast.decl) (titems : Ast.test_item list) : Ir.test_decl =
  let scope : (string * bkind) list ref = ref [] in
  let refty = make_refty ctx scope in
  let constructions = ref [] in
  let stubs = ref [] in
  let calls = ref [] in
  let expects = ref [] in
  let lookup name span =
    match List.assoc_opt name !scope with
    | Some k -> Some k
    | None ->
        V.report ctx
          (err Error_codes.test_binding_unknown span
             "unknown binding '%s'; a test references bindings declared above"
             name);
        None
  in
  List.iter
    (fun (item : Ast.test_item) ->
      match item with
      | Ast.TiConstruct { bind; bind_span; entry; entry_span; fields; _ } -> (
          match V.decl_by_name ctx entry with
          | Some ({ dkind = Ast.DStruct { members; ops = _ :: _; _ }; _ } as e)
            ->
              let values =
                List.filter_map
                  (fun (name, name_span, fv) ->
                    match
                      List.find_opt
                        (fun (m : Ast.member) -> String.equal m.Ast.mname name)
                        members
                    with
                    | None ->
                        V.report ctx
                          (err Error_codes.test_value_invalid name_span
                             "entry '%s' has no field '%s'" entry name);
                        None
                    | Some m ->
                        Some (name, V.encode_value ctx ~refty m.Ast.mtype fv))
                  fields
              in
              constructions :=
                { Ir.tc_binding = bind; tc_entry = entry; tc_values = values }
                :: !constructions;
              add_binding ctx scope bind bind_span (Bentry e)
          | Some _ ->
              V.report ctx
                (err Error_codes.test_value_invalid entry_span
                   "'%s' is not an entry (a struct that declares operations)"
                   entry)
          | None ->
              V.report ctx
                (err Error_codes.test_value_invalid entry_span
                   "unknown entry '%s'" entry))
      | Ast.TiStub { bind; target; value; _ } -> (
          match lookup target.st_binding target.st_span with
          | Some (Bentry e) -> (
              match
                List.find_opt
                  (fun (o : Ast.decl) -> String.equal o.Ast.dname target.st_op)
                  (entry_ops e)
              with
              | None ->
                  V.report ctx
                    (err Error_codes.test_op_unknown target.st_span
                       "entry '%s' declares no operation '%s'" e.Ast.dname
                       target.st_op)
              | Some op -> (
                  let dep =
                    match target.st_dep with
                    | "http" ->
                        if V.find_trait "http" op.Ast.dtraits = None then (
                          V.report ctx
                            (err Error_codes.test_dep_invalid target.st_span
                               "operation '%s' has no http dependency; it is \
                                not bound to @http"
                               target.st_op);
                          None)
                        else Some Ir.Dep_http
                    | "impl" ->
                        if not (impl_covers ctx.V.decls ~entry:e ~op) then (
                          V.report ctx
                            (err Error_codes.test_dep_invalid target.st_span
                               "operation '%s' has no impl dependency; no 'ext \
                                impl' covers it"
                               target.st_op);
                          None)
                        else Some Ir.Dep_impl
                    | other ->
                        V.report ctx
                          (err Error_codes.test_dep_invalid target.st_span
                             "unknown dependency '%s'; a stub replaces the \
                              declared '.http' or '.impl' dependency"
                             other);
                        None
                  in
                  match dep with
                  | None -> ()
                  | Some dep ->
                      let answers = stub_answers ctx ~refty dep op value in
                      stubs :=
                        {
                          Ir.ts_binding = Option.map fst bind;
                          ts_client = target.st_binding;
                          ts_op = target.st_op;
                          ts_dep = dep;
                          ts_answers = answers;
                        }
                        :: !stubs;
                      Option.iter
                        (fun (b, bspan) ->
                          add_binding ctx scope b bspan (Bstub { dep }))
                        bind))
          | Some _ ->
              V.report ctx
                (err Error_codes.test_binding_unknown target.st_span
                   "'%s' is not a construction binding" target.st_binding)
          | None -> ())
      | Ast.TiCall { bind; bind_span; recv; recv_span; op; op_span; input; _ }
        -> (
          match lookup recv recv_span with
          | Some (Bentry e) -> (
              match
                List.find_opt
                  (fun (o : Ast.decl) -> String.equal o.Ast.dname op)
                  (entry_ops e)
              with
              | None ->
                  V.report ctx
                    (err Error_codes.test_op_unknown op_span
                       "entry '%s' declares no operation '%s'" e.Ast.dname op)
              | Some op_decl ->
                  let call_input =
                    match (op_input op_decl, input) with
                    | Some ty, Some v -> Some (V.encode_value ctx ~refty ty v)
                    | Some _, None ->
                        V.report ctx
                          (err Error_codes.test_value_invalid op_span
                             "operation '%s' takes an input" op);
                        None
                    | None, Some v ->
                        V.report ctx
                          (err Error_codes.test_value_invalid (V.value_span v)
                             "operation '%s' takes no input" op);
                        None
                    | None, None -> None
                  in
                  calls :=
                    {
                      Ir.call_binding = bind;
                      call_client = recv;
                      call_op = op;
                      call_input;
                    }
                    :: !calls;
                  add_binding ctx scope bind bind_span (Bcall { op = op_decl }))
          | Some _ ->
              V.report ctx
                (err Error_codes.test_binding_unknown recv_span
                   "'%s' is not a construction binding; a call goes through \
                    the constructed entry"
                   recv)
          | None -> ())
      | Ast.TiExpect { subject; subject_span; requests; pattern; _ } -> (
          match lookup subject subject_span with
          | None -> ()
          | Some k -> (
              match (requests, k) with
              | true, Bstub { dep } ->
                  Option.iter
                    (fun e -> expects := e :: !expects)
                    (requests_expect ctx ~refty dep subject pattern)
              | true, _ ->
                  V.report ctx
                    (err Error_codes.test_requests_subject_invalid subject_span
                       "'%s' is not a stub binding; '.requests' reads what \
                        crossed a stubbed dependency"
                       subject)
              | false, Bstub _ ->
                  V.report ctx
                    (err Error_codes.test_pattern_invalid subject_span
                       "a stub binding is asserted through '.requests'")
              | false, Bentry _ ->
                  Option.iter
                    (fun p ->
                      expects :=
                        Ir.Expect_outcome
                          { ex_subject = subject; ex_pattern = p }
                        :: !expects)
                    (construction_pattern ctx ~refty pattern)
              | false, Bcall { op } ->
                  Option.iter
                    (fun p ->
                      expects :=
                        Ir.Expect_outcome
                          { ex_subject = subject; ex_pattern = p }
                        :: !expects)
                    (call_pattern ctx ~refty op pattern))))
    titems;
  let has_expect =
    List.exists (function Ast.TiExpect _ -> true | _ -> false) titems
  in
  if not has_expect then
    V.report ctx
      (err Error_codes.test_expect_missing d.Ast.dname_span
         "test %S has no expect; a test asserts at least one outcome"
         d.Ast.dname);
  {
    Ir.t_name = d.Ast.dname;
    t_constructions = List.rev !constructions;
    t_stubs = List.rev !stubs;
    t_calls = List.rev !calls;
    t_expects = List.rev !expects;
  }

let check_decls ~(imports : Ast.import list) (decls : Ast.decl list) :
    Ir.test_decl list * Diagnostic.t list =
  let diags = ref [] in
  let ctx = V.make_ctx ~imports ~decls ~diags in
  let tests =
    List.filter_map
      (fun (d : Ast.decl) ->
        match d.Ast.dkind with
        | Ast.DTest { titems } -> Some (check_test ctx d titems)
        | _ -> None)
      decls
  in
  (tests, List.rev !diags)
