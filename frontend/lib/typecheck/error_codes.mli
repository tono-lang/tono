(* Stable typecheck diagnostic codes. Each maps to one well-defined failure so
   tooling and tests can pin behaviour by code rather than message text. *)

val unknown_type : string
val duplicate_shape : string

(* TC0003 is intentionally unused: type-parameter scope is resolved during
   lowering, so an out-of-scope name surfaces as [unknown_type]. *)
val generic_arity_mismatch : string
val non_generic_applied : string

(* TC0006 (bounds_not_supported) is a parse-level rejection; no constant here. *)
val nullability_conflict : string
val enum_value_duplicate : string
val enum_backing_mismatch : string
val constraint_type_mismatch : string
val constraint_malformed : string
val default_type_mismatch : string
val default_violates_constraint : string
val unresolved_operation_ref : string
val error_status_missing : string
val error_code_invalid : string
val error_discrimination_ambiguous : string
val async_takes_no_arguments : string
val http_label_unmatched : string
val http_payload_conflict : string
val http_map_binding : string
val http_label_nullable : string

(* Module system: unknown import qualifier, non-[pub] reference, a cycle in the
   module import graph (which must be a DAG), and two imports colliding on one
   qualifier. *)
val unknown_import : string
val not_exported : string
val module_cycle : string
val duplicate_import : string

(* Extension-model checks: an unknown hook slot, an unsupported binding language,
   a kind/signature-shape violation, an extension with no binding, a binding that
   is not a "file#symbol" reference, a duplicate extension name, and a language
   bound twice within one extension (which would collapse to one wire key). *)
val ext_unknown_hook_slot : string
val ext_binding_language_invalid : string
val ext_signature_rule : string
val ext_binding_missing : string
val ext_binding_malformed : string
val ext_duplicate : string
val ext_binding_duplicate_language : string

(* Entry model: closed boundaries between entry/config and wire, declared value
   sources, table selection, composition, and protocol trait positions. *)
val entry_wire_boundary : string
val source_position_invalid : string
val source_dead : string
val field_unresolvable : string
val field_ref_unknown : string
val resolution_cycle : string
val match_invalid : string
val match_not_exhaustive : string
val bind_invalid : string
val entry_endpoint_missing : string
val protocol_trait_invalid : string
val transform_unknown : string
val entry_shape_invalid : string
val duplicate_trait : string

(* Bespoke operation implementations: an impl naming no implementable operation
   or an ambiguous one, an operation implemented twice or not at all, and the raw
   response form on a kind that cannot carry it. *)
val ext_impl_unknown_op : string
val ext_impl_ambiguous_op : string
val op_implementation_conflict : string
val op_implementation_missing : string
val ext_raw_rule : string

(* A declared error a raw implementation could never select: the raw path
   discriminates by @errorCode alone, so an error without one is unreachable. *)
val raw_error_unreachable : string
