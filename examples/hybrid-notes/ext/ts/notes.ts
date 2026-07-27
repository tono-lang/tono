// The bespoke half of the hybrid SDK: the two operations the service does not
// serve over HTTP. The generated serde file imports these relative to itself,
// so drop the ext/ directory next to the generated SDK files.
//
// The store here is a deterministic stand-in for whatever the real thing is
// (a legacy library, a cache, a crypto module). It is written to be exercised
// by the conformance vectors, so every branch is reachable from an input, and
// it agrees with the Go implementation case for case.

import type { Outcome } from "@tono/ext-runtime-ts";

import type { Settings } from "../../notes_serde";
import { type Note, type NoteRef, OverloadedError } from "../../notes";

// storeSave is the typed form: it speaks the operation's own types, so it owns
// the mapping and reports a declared failure as the declared value. Throwing
// OverloadedError crosses the generated boundary untouched, retryable() and
// all; throwing anything else becomes a ContractError naming the operation.
export async function storeSave(settings: Settings, input: Note): Promise<Note> {
  switch (input.id) {
    case "overload":
      throw new OverloadedError(
        { message: "the store is shedding load" },
        JSON.stringify({ message: "the store is shedding load" }),
      );
    case "boom":
      throw new Error("the store panicked");
  }
  return { id: input.id, body: input.body, modified: `saved-by-${settings.apiToken}` };
}

// storeArchive is the raw form: it returns the payload the generated glue
// integrates. On success the body is decoded strictly into Note; on failure the
// code is matched against the operation's declared error codes. Neither side
// writes mapping code.
export async function storeArchive(settings: Settings, payload: string): Promise<Outcome> {
  const ref = JSON.parse(payload) as NoteRef;
  switch (ref.id) {
    case "overload":
      return failure("overloaded", '{"message":"the store is shedding load"}');
    case "unknown-failure":
      // No declared error carries this code, so the glue falls back to the
      // generic API error rather than guessing.
      return failure("quota_exceeded", '{"message":"out of quota"}');
    case "corrupt":
      // A success payload missing a required member: the glue rejects it with
      // the path of the member the store omitted.
      return { success: true, code: "", body: '{"id":"corrupt"}' };
    case "boom":
      throw new Error("the store panicked");
  }
  const body = JSON.stringify({
    id: ref.id,
    body: "archived",
    updated_at: `archived-by-${settings.apiToken}`,
  });
  return { success: true, code: "", body };
}

function failure(code: string, body: string): Outcome {
  return { success: false, code, body };
}
