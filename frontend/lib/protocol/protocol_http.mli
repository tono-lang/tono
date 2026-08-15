(* The HTTP Protocol: resolves each operation into a typed wire binding
   ([Ir.wire_binding]) a target reads directly to emit its own transport.
   Protocol knows transport (HTTP bindings) but not language; the binding it
   produces is pure data.

   Binding assignment is a materialization pass over the IR (it reads the trait
   bag the frontend already lowered). It assumes the HTTP annotations are valid;
   [Check_http] reports malformed bindings at the AST level, where source spans
   still exist. *)

(* Where an output member is read from in the HTTP response. *)
type response_part = Response_header of string | Response_status_code

(* A value position in a protocol trait: a literal, an entry-field reference,
   or a template mixing literal runs with entry-field placeholders. A target
   reads these directly; there is no runtime left to interpret them
   generically. *)
type value_expr =
  | Vlit of Ir.json
  | Vfield of string list
  | Vparam of string list
    (* segments into the op's declared parameter; [] is the whole value *)
  | Vtemplate of Ir.template_part list
  | Vctor of (string * value_expr) list
    (* @body's ctor mapper: field name -> value *)
  | Vcall of call_value
(* an extern call read as a @header/@query/@body value *)

and call_value = { vc_ns : string; vc_fn : string; vc_args : call_arg_value list }

and call_arg_value =
  | Cv_field of string list
  | Cv_param of string list
  | Cv_lit of Ir.json
  | Cv_ctor of (string * call_arg_value) list
  | Cv_request
(* the reserved ".request" marker: the canonical, already-assembled
       request, legal only here *)

(* The language-agnostic wire form of one operation, en route to
   [Ir.wire_binding]. The endpoint, timeout, and retry refs only arise on
   operations declared in an entry body (a loose op leaves them empty);
   request_headers carries op-level @header declarations, which either kind of
   operation may make. *)
type resolution = {
  http_method : string;
  uri : value_expr; (* literal, entry/param reference, or template *)
  response_bindings : (string * response_part) list;
  success : (int * Ir.tref option) list; (* status -> output type, if any *)
  (* Declared errors ((status, shape id, @errorCode, @retryable)) are not
     carried here: the generated SDK's own decode<Op>Error + .retryable()
     pair is the single owner of that classification, consumed by the
     runtime's retry loop through a predicate the generated call site builds.
     Duplicating it on the descriptor let the two drift apart. *)
  endpoint : value_expr option; (* @http endpoint: literal, ref, or template *)
  request_headers : (Ir.template_part list * value_expr) list;
      (* @header(key, value): key template -> value *)
  query : (Ir.template_part list * value_expr) list;
      (* @query(key, value): key template -> value *)
  body : value_expr option;
      (* @body(...): absent means the operation sends no body *)
  timeout : string list option;
      (* @timeout(.field): encodes as the runtime's value-source form
         {"ref": "<dotted field path>"} *)
  retry : string list option;
      (* @retry(.field): encodes as {"max": {"ref": "<dotted field path>"}} *)
}

(* Resolve one operation shape against a shape lookup. Returns [None] for an
   operation with no [@http] trait (a purely local op carries no descriptor). *)
val resolve_op :
  (Ir.shape_id -> Ir.shape option) -> Ir.shape -> resolution option

(* The resolution as a first-class IR value: minus the errors array (redundant
   with the operation's own [errors] field) and the success tref (every
   runtime already discards it), with [timeout]/[retry] kept as a plain
   entry-field path. *)
val to_ir_binding : resolution -> Ir.wire_binding

(* Attach the resolved binding to every operation of a module: a typed [wire]
   field on the operation's [Ir.shape_kind] for direct consumption. Leaves all
   other shapes untouched. *)
val resolve_module : Ir.module_ -> Ir.module_
