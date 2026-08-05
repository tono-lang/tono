(* The HTTP Protocol: resolves each operation into an opaque wire descriptor the
   Target embeds verbatim and the runtime interprets. Protocol knows transport
   (HTTP bindings) but not language; the descriptor it produces is pure data.

   Binding assignment is a materialization pass over the IR (it reads the trait
   bag the frontend already lowered). It assumes the HTTP annotations are valid;
   [Check_http] reports malformed bindings at the AST level, where source spans
   still exist. *)

(* Where an input member travels in the HTTP request. *)
type part =
  | Label (* path parameter: substitutes {name} in the uri *)
  | Query of string (* query-string parameter with this name *)
  | Header of string (* HTTP header with this name *)
  | Body (* a field inside the JSON request body (default) *)
  | Payload (* this member is the whole body, no envelope *)

(* Where an output member is read from in the HTTP response. *)
type response_part = Response_header of string | Response_status_code

(* A value position in a protocol trait: a literal, an entry-field reference,
   or a template mixing literal runs with entry-field placeholders. The runtime
   interprets these verbatim; it never learns the field taxonomy. *)
type value_expr =
  | Vlit of Ir.json
  | Vfield of string list
  | Vtemplate of Ir.template_part list

(* The opaque, language-agnostic wire form of one operation. The Target embeds
   its JSON encoding without interpreting any field. The endpoint, timeout, and
   retry refs only arise on operations declared in an entry body (a loose op
   leaves them empty); request_headers carries op-level @header declarations,
   which either kind of operation may make. *)
type wire_descriptor = {
  http_method : string;
  uri : string; (* path template with {name} and {.field} placeholders *)
  bindings : (string * part) list; (* input member -> request part *)
  response_bindings : (string * response_part) list;
  success : (int * Ir.tref option) list; (* status -> output type, if any *)
  (* Declared errors ((status, shape id, @errorCode, @retryable)) are not
     carried here: the generated SDK's own decode<Op>Error + .retryable()
     pair is the single owner of that classification, consumed by the
     runtime's retry loop through a predicate the generated call site builds.
     Duplicating it on the descriptor let the two drift apart. *)
  endpoint : string list option; (* @http endpoint: entry-field path *)
  request_headers : (Ir.template_part list * value_expr) list;
      (* @header(key, value): key template -> value *)
  timeout : string list option;
      (* @timeout(.field): encodes as the runtime's value-source form
         {"ref": "<dotted field path>"} *)
  retry : string list option;
      (* @retry(.field): encodes as {"max": {"ref": "<dotted field path>"}} *)
}

(* Resolve one operation shape against a shape lookup. Returns [None] for an
   operation with no [@http] trait (a purely local op carries no descriptor). *)
val resolve_op :
  (Ir.shape_id -> Ir.shape option) -> Ir.shape -> wire_descriptor option

(* The JSON encoding embedded, opaque, in the generated stub. Kept, unchanged,
   for backward compatibility until every emitter reads [Ir.wire_binding]
   directly instead. *)
val encode : wire_descriptor -> Ir.json

(* The typed counterpart of [encode]: the same resolution as a first-class IR
   value instead of an opaque blob, minus the errors array (redundant with the
   operation's own [errors] field) and the success tref (every runtime already
   discards it), with [timeout]/[retry] kept as a plain entry-field path. *)
val to_ir_binding : wire_descriptor -> Ir.wire_binding

(* Attach the resolved binding to every operation of a module: a typed [wire]
   field on the operation's [Ir.shape_kind] for direct consumption, plus
   (temporarily, until callers migrate off it) the same resolution serialized
   as an opaque [wire_descriptor] trait for backward compatibility. Leaves all
   other shapes untouched. *)
val resolve_module : Ir.module_ -> Ir.module_
