// The Go conformance driver: runs every vector through the generated client
// and prints one classified result per case. The TypeScript driver prints the
// same shapes from the same vectors, so comparing the two outputs is what
// proves the implementations agree; comparing either against the vectors'
// declared expectations is what proves they are both right.
//
// The client is driven, not the bespoke symbol directly: the point of a vector
// is the behavior a caller sees, which includes the generated glue.

package main

import (
	"context"
	"encoding/json"
	"fmt"
	"os"

	notes "example.com/notes"
)

type vectorFile struct {
	Operation string   `json:"operation"`
	Cases     []vecCase `json:"cases"`
}

type vecCase struct {
	Name  string          `json:"name"`
	Input json.RawMessage `json:"input"`
}

func main() {
	if len(os.Args) < 2 {
		fail(fmt.Errorf("usage: driver <vectors.json>..."))
	}
	client, err := notes.New()
	if err != nil {
		fail(err)
	}
	results := []map[string]any{}
	for _, path := range os.Args[1:] {
		file := readVectors(path)
		for _, c := range file.Cases {
			results = append(results, run(client, file.Operation, c))
		}
	}
	out, err := json.Marshal(results)
	if err != nil {
		fail(err)
	}
	fmt.Println(string(out))
}

func run(client *notes.Client, operation string, c vecCase) map[string]any {
	ctx := context.Background()
	var result map[string]any
	switch operation {
	case "save_note":
		var input notes.Note
		decode(c.Input, &input)
		out, err := client.SaveNote(ctx, input)
		result = classify(wire(out), err)
	case "archive_note":
		var input notes.NoteRef
		decode(c.Input, &input)
		out, err := client.ArchiveNote(ctx, input)
		result = classify(wire(out), err)
	default:
		fail(fmt.Errorf("unknown operation %q", operation))
	}
	result["name"] = c.Name
	return result
}

// classify reduces the call to the closed vocabulary both drivers share. The
// declared errors are named explicitly: this SDK has one, and spelling it out
// is what makes the typed-passthrough claim checkable.
func classify(value any, err error) map[string]any {
	if err == nil {
		return map[string]any{"outcome": "ok", "value": value}
	}
	switch e := err.(type) {
	case *notes.ValidationError:
		fields := []string{}
		for _, v := range e.Violations {
			fields = append(fields, v.Field)
		}
		return map[string]any{"outcome": "validation", "fields": fields}
	case *notes.Overloaded:
		return map[string]any{
			"outcome":   "declared",
			"code":      e.Error(),
			"retryable": e.Retryable(),
			"data":      map[string]any{"message": e.Message},
		}
	case *notes.DecodeError:
		return map[string]any{"outcome": "decode", "path": e.Path}
	case *notes.ContractError:
		return map[string]any{"outcome": "contract", "contract": e.ContractName}
	case *notes.APIError:
		return map[string]any{"outcome": "api", "status": e.Status, "body": e.Body}
	}
	return map[string]any{"outcome": "unclassified", "error": err.Error()}
}

// wire renders a note in its serialized form, so a language's own field
// spelling (@rename) never leaks into the comparison.
func wire(n notes.Note) any {
	b, err := json.Marshal(n)
	if err != nil {
		fail(err)
	}
	var out any
	decode(b, &out)
	return out
}

func readVectors(path string) vectorFile {
	b, err := os.ReadFile(path)
	if err != nil {
		fail(err)
	}
	var file vectorFile
	decode(b, &file)
	return file
}

func decode(b []byte, into any) {
	if err := json.Unmarshal(b, into); err != nil {
		fail(err)
	}
}

func fail(err error) {
	fmt.Fprintln(os.Stderr, err)
	os.Exit(1)
}
