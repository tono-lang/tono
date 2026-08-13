(* Overflow fixtures split out of [Ir_corpus] to keep that file within the
   size budget. [Ir_corpus] keys these into the fixture corpus. *)

open Tono_frontend

(* Small local helpers, duplicated from [Ir_corpus] rather than referenced:
   dune's wrapped-library convention makes [Ir_corpus] (the module sharing the
   library's name) the aggregator other modules feed into, never the other
   way around. *)
let member ?(required = true) ?default ?(constraints = []) ?(traits = []) name
    target : Ir.member =
  { name; target; required; default; constraints; traits }

let ref_ id args = Ir.Ref (id, args)
let string_t = Ir.Prim Ir.String

(* Example: the resolved wire binding an operation carries directly for a
   target to read: every wire_value/response-part kind (including the
   @body ctor's object form), a uri template mixing all three placeholder
   forms, and the entry-scoped endpoint/timeout/retry refs. *)
let resolved_wire : Ir.model =
  let charge : Ir.shape =
    {
      id = "payments#Charge";
      kind = Ir.Structure { params = []; members = [ member "id" string_t ] };
      traits = [];
    }
  in
  let get_charge : Ir.shape =
    {
      id = "payments#client.get_charge";
      kind =
        Ir.Operation
          {
            input = Some (ref_ "payments#Charge" []);
            input_name = None;
            output = Some (ref_ "payments#Charge" []);
            errors = [];
            wire =
              Some
                {
                  Ir.wb_method = "GET";
                  wb_uri =
                    Ir.Wire_template
                      [
                        Ir.Tpl_lit "/charges/";
                        Ir.Tpl_input "id";
                        Ir.Tpl_lit "/";
                        Ir.Tpl_field [ "endpoint_suffix" ];
                      ];
                  wb_body =
                    Some
                      (Ir.Wire_object
                         [
                           ("id", Ir.Wire_param []);
                           ("extra", Ir.Wire_field [ "extra" ]);
                         ]);
                  wb_response_bindings =
                    [
                      ("trace_id", Ir.Wire_response_header "X-Trace-Id");
                      ("status", Ir.Wire_response_status_code);
                    ];
                  (* [Protocol_http.to_ir_binding] only ever produces a
                     singleton: this fixture exercises the JSON shape space,
                     not a realistic resolver output. *)
                  wb_success = [ 200; 202 ];
                  wb_endpoint = Some (Ir.Wire_field [ "endpoint" ]);
                  wb_request_headers =
                    [
                      ( [ Ir.Tpl_lit "X-Client" ],
                        Ir.Wire_field [ "client_name" ] );
                      ([ Ir.Tpl_lit "X-Fixed" ], Ir.Wire_lit (`String "v1"));
                      ( [ Ir.Tpl_lit "X-Combo" ],
                        Ir.Wire_template
                          [ Ir.Tpl_lit "v-"; Ir.Tpl_field [ "client_name" ] ] );
                    ];
                  wb_query =
                    [
                      ([ Ir.Tpl_lit "limit" ], Ir.Wire_field [ "default_limit" ]);
                    ];
                  wb_timeout = Some [ "timeout" ];
                  wb_retry = Some [ "settings"; "max_retries" ];
                };
            impl_call = None;
          };
      traits = [];
    }
  in
  {
    tono_ir_version = Ir_json.current_ir_version;
    modules =
      [
        {
          mod_name = "payments";
          shapes = [ charge ];
          operations = [ get_charge ];
          extensions = [];
          ext_libs = [];
          tests = [];
        };
      ];
  }
