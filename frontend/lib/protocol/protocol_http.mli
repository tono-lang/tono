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

(* The opaque, language-agnostic wire form of one operation. The Target embeds
   its JSON encoding without interpreting any field. *)
type wire_descriptor = {
  http_method : string;
  uri : string; (* path template with {name} placeholders *)
  bindings : (string * part) list; (* input member -> request part *)
  response_bindings : (string * response_part) list;
  success : (int * Ir.tref option) list; (* status -> output type, if any *)
  errors : (int * Ir.shape_id * string option) list;
      (* status -> error shape id; 3rd is the @errorCode body field, if any *)
}

(* Resolve one operation shape against a shape lookup. Returns [None] for an
   operation with no [@http] trait (a purely local op carries no descriptor). *)
val resolve_op :
  (Ir.shape_id -> Ir.shape option) -> Ir.shape -> wire_descriptor option

(* The JSON encoding embedded, opaque, in the generated stub. *)
val encode : wire_descriptor -> Ir.json

(* Attach the resolved descriptor to every operation of a module as a synthesized
   [wire_descriptor] trait, leaving all other shapes untouched. Protocol is an
   IR -> IR annotation step: the descriptor rides the trait bag, so the core IR
   stays protocol-agnostic and the wire format needs no version bump. *)
val resolve_module : Ir.module_ -> Ir.module_
