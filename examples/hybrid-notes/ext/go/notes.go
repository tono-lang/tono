// The bespoke half of the hybrid SDK: the two operations the service does not
// serve over HTTP. Go calls a bound symbol unqualified, from inside the
// generated package, so this file is dropped next to the generated sources
// (the binding path is a "put it here" instruction, not an import path).
//
// The store here is a deterministic stand-in for whatever the real thing is
// (a legacy library, a cache, a crypto module). It is written to be exercised
// by the conformance vectors, so every branch is reachable from an input.

package notes

import (
	"context"
	"encoding/json"
	"errors"

	tonoext "github.com/tono-lang/tono/runtimes/ext-go"
)

// StoreSave is the typed form: it speaks the operation's own types, so it owns
// the mapping and reports a declared failure as the declared value. Returning
// *Overloaded crosses the generated boundary untouched, Retryable() and all;
// returning anything else becomes a ContractError naming the operation.
func StoreSave(ctx context.Context, s *Settings, input Note) (Note, error) {
	switch input.ID {
	case "overload":
		return Note{}, &Overloaded{Message: "the store is shedding load"}
	case "boom":
		return Note{}, errors.New("the store panicked")
	}
	return Note{ID: input.ID, Body: input.Body, Modified: "saved-by-" + s.APIToken}, nil
}

// StoreArchive is the raw form: it returns the payload the generated glue
// integrates. On success the body is decoded strictly into Note; on failure the
// code is matched against the operation's declared error codes. Neither side
// writes mapping code.
func StoreArchive(ctx context.Context, s *Settings, payload []byte) (tonoext.Outcome, error) {
	var ref NoteRef
	if err := json.Unmarshal(payload, &ref); err != nil {
		return tonoext.Outcome{}, err
	}
	switch ref.ID {
	case "overload":
		return failure("overloaded", `{"message":"the store is shedding load"}`), nil
	case "unknown-failure":
		// No declared error carries this code, so the glue falls back to the
		// generic API error rather than guessing.
		return failure("quota_exceeded", `{"message":"out of quota"}`), nil
	case "corrupt":
		// A success payload missing a required member: the glue rejects it with
		// the path of the member the store omitted.
		return tonoext.Outcome{Success: true, Body: []byte(`{"id":"corrupt"}`)}, nil
	case "boom":
		return tonoext.Outcome{}, errors.New("the store panicked")
	}
	body, err := json.Marshal(Note{
		ID:       ref.ID,
		Body:     "archived",
		Modified: "archived-by-" + s.APIToken,
	})
	if err != nil {
		return tonoext.Outcome{}, err
	}
	return tonoext.Outcome{Success: true, Body: body}, nil
}

func failure(code string, body string) tonoext.Outcome {
	return tonoext.Outcome{Success: false, Code: code, Body: []byte(body)}
}
