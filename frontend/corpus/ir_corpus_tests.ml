(* The declared-tests corpus example (v7), split out of [Ir_corpus] to keep
   both files within the size budget. [Ir_corpus] keys it into the fixture
   corpus as "declared_tests". *)

open Tono_frontend

let member ?(required = true) ?(traits = []) name target : Ir.member =
  { name; target; required; default = None; constraints = []; traits }

let trait id value : Ir.trait = { trait_id = id; value }
let prim p = Ir.Prim p
let ref_ id args = Ir.Ref (id, args)
let string_t = prim Ir.String

(* Example: declared tests (v7). A GitHub-like module: one entry whose
   [get_user] goes through @http and whose [save_note] is a bespoke impl, plus
   three tests -- a hermetic http fixture (stubbed response, struct pattern,
   request assertions), an impl test (declared error crossing typed), and a
   live test (no stub; the opt-in suite). This fixture is the wire contract the
   Rust mirror reproduces. *)
let declared_tests : Ir.model =
  let field ?(sources = []) name target : Ir.entry_field =
    {
      ef_name = name;
      ef_target = target;
      ef_sources = sources;
      ef_format = None;
      ef_transforms = [];
      ef_select = None;
      ef_binds = [];
      ef_constraints = [];
      ef_traits = [];
    }
  in
  let i32 = prim (Ir.int_prim ~bits:32 ~signed:true) in
  let user : Ir.shape =
    {
      id = "github#user";
      kind =
        Ir.Structure
          {
            params = [];
            members =
              [
                member "login" string_t;
                member "id" i32;
                member "name" string_t ~required:false;
                member "bio" string_t ~required:false;
                member "public_repos" i32;
                member "created_at" string_t;
              ];
          };
      traits = [ trait "pub" `Null ];
    }
  in
  let user_ref : Ir.shape =
    {
      id = "github#user_ref";
      kind =
        Ir.Structure
          {
            params = [];
            members =
              [ member "username" string_t ~traits:[ trait "httpLabel" `Null ] ];
          };
      traits = [ trait "pub" `Null ];
    }
  in
  let note : Ir.shape =
    {
      id = "github#note";
      kind =
        Ir.Structure
          {
            params = [];
            members =
              [
                member "id" string_t;
                member "body" string_t;
                member "updated_at" string_t;
              ];
          };
      traits = [ trait "pub" `Null ];
    }
  in
  let overloaded : Ir.shape =
    {
      id = "github#overloaded";
      kind =
        Ir.Structure { params = []; members = [ member "message" string_t ] };
      traits =
        [
          trait "status" (`List [ `Int 529 ]);
          trait "errorCode" (`List [ `String "code"; `String "overloaded" ]);
        ];
    }
  in
  let get_user : Ir.shape =
    {
      id = "github#client.get_user";
      kind =
        Ir.Operation
          {
            input = Some (ref_ "github#user_ref" []);
            input_name = None;
            output = Some (ref_ "github#user" []);
            errors = [];
            wire = None;
          };
      traits =
        [
          trait "http"
            (`Assoc
               [
                 ("method", `String "GET");
                 ("path", `String "/users/{username}");
                 ("endpoint", `Assoc [ ("field", `List [ `String "endpoint" ]) ]);
               ]);
        ];
    }
  in
  let save_note : Ir.shape =
    {
      id = "github#client.save_note";
      kind =
        Ir.Operation
          {
            input = Some (ref_ "github#note" []);
            input_name = None;
            output = Some (ref_ "github#note" []);
            errors = [ ref_ "github#overloaded" [] ];
            wire = None;
          };
      traits = [];
    }
  in
  let client : Ir.shape =
    {
      id = "github#client";
      kind =
        Ir.Entry
          {
            fields =
              [
                field "api_token" string_t ~sources:[ Ir.Arg ];
                field "endpoint" string_t
                  ~sources:
                    [
                      Ir.Env (Ir.Env_name "GITHUB_ENDPOINT");
                      Ir.Default (`String "https://api.github.com");
                    ];
              ];
            operations = [ get_user; save_note ];
          };
      traits = [ trait "pub" `Null ];
    }
  in
  let save_note_impl : Ir.extension =
    {
      ext_name = "client.save_note";
      ext_kind = Ir.Impl;
      ext_sig = None;
      ext_raw = false;
      ext_bindings =
        [
          ("go", "ext/go/notes.go#SaveNote"); ("ts", "ext/ts/notes.ts#saveNote");
        ];
      ext_conformance = None;
    }
  in
  let octocat_body =
    "{\"login\":\"octocat\",\"id\":583231,\"name\":\"The \
     Octocat\",\"bio\":null,\"public_repos\":8,\"created_at\":\"2011-01-25T18:44:36Z\"}"
  in
  let hermetic : Ir.test_decl =
    {
      t_name = "the profile github actually answers decodes as declared";
      t_constructions =
        [ { tc_binding = "c"; tc_entry = "client"; tc_values = [] } ];
      t_stubs =
        [
          {
            ts_binding = Some "s";
            ts_client = "c";
            ts_op = "get_user";
            ts_dep = Ir.Dep_http;
            ts_answers =
              [
                Ir.Answer_http
                  {
                    ans_status = 200;
                    ans_headers = [];
                    ans_body = octocat_body;
                  };
              ];
          };
        ];
      t_calls =
        [
          {
            call_binding = "got";
            call_client = "c";
            call_op = "get_user";
            call_input = Some (`Assoc [ ("username", `String "octocat") ]);
          };
        ];
      t_expects =
        [
          Ir.Expect_outcome
            {
              ex_subject = "got";
              ex_pattern =
                Ir.P_struct
                  {
                    ps_shape = "user";
                    ps_open = true;
                    ps_fields =
                      [
                        ("login", Ir.Fp_pat (Ir.P_eq (`String "octocat")));
                        ("id", Ir.Fp_pat (Ir.P_eq (`Int 583231)));
                        ("bio", Ir.Fp_absent);
                        ("name", Ir.Fp_present);
                      ];
                  };
            };
          Ir.Expect_requests
            {
              ex_subject = "s";
              ex_requests =
                [
                  {
                    rp_open = true;
                    rp_fields =
                      [
                        ("method", Ir.Fp_pat (Ir.P_eq (`String "GET")));
                        ("path", Ir.Fp_pat (Ir.P_eq (`String "/users/octocat")));
                      ];
                    rp_headers =
                      Some
                        [
                          ( "accept",
                            Ir.Fp_pat (Ir.P_eq (`String "application/json")) );
                        ];
                  };
                ];
            };
        ];
    }
  in
  let impl_test : Ir.test_decl =
    {
      t_name = "the glue guards the store's declared failure";
      t_constructions =
        [
          {
            tc_binding = "c";
            tc_entry = "client";
            tc_values = [ ("api_token", `String "t0") ];
          };
        ];
      t_stubs =
        [
          {
            ts_binding = None;
            ts_client = "c";
            ts_op = "save_note";
            ts_dep = Ir.Dep_impl;
            ts_answers =
              [
                Ir.Answer_error
                  {
                    ans_shape = "overloaded";
                    ans_data =
                      `Assoc [ ("message", `String "simulated shedding") ];
                  };
              ];
          };
        ];
      t_calls =
        [
          {
            call_binding = "saved";
            call_client = "c";
            call_op = "save_note";
            call_input =
              Some
                (`Assoc
                   [
                     ("id", `String "n1");
                     ("body", `String "hello");
                     ("updated_at", `String "");
                   ]);
          };
        ];
      t_expects =
        [
          Ir.Expect_outcome
            {
              ex_subject = "saved";
              ex_pattern =
                Ir.P_error
                  {
                    pe_shape = "overloaded";
                    pe_open = false;
                    pe_fields =
                      [
                        ( "message",
                          Ir.Fp_pat (Ir.P_eq (`String "simulated shedding")) );
                      ];
                  };
            };
        ];
    }
  in
  let live : Ir.test_decl =
    {
      t_name = "the spec still matches the real api";
      t_constructions =
        [ { tc_binding = "c"; tc_entry = "client"; tc_values = [] } ];
      t_stubs = [];
      t_calls =
        [
          {
            call_binding = "got";
            call_client = "c";
            call_op = "get_user";
            call_input = Some (`Assoc [ ("username", `String "torvalds") ]);
          };
        ];
      t_expects =
        [
          Ir.Expect_outcome
            {
              ex_subject = "got";
              ex_pattern =
                Ir.P_struct
                  {
                    ps_shape = "user";
                    ps_open = true;
                    ps_fields =
                      [
                        ("login", Ir.Fp_pat (Ir.P_eq (`String "torvalds")));
                        ("id", Ir.Fp_pat (Ir.P_eq (`Int 1024025)));
                        ("name", Ir.Fp_present);
                      ];
                  };
            };
        ];
    }
  in
  {
    tono_ir_version = Ir_json.current_ir_version;
    modules =
      [
        {
          mod_name = "github";
          shapes = [ user; user_ref; note; overloaded; client ];
          operations = [];
          extensions = [ save_note_impl ];
          tests = [ hermetic; impl_test; live ];
        };
      ];
  }
