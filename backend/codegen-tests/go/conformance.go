//go:build conformance

// The conformance driver: read a JSON array of wire documents from stdin,
// decode and re-encode each into the generated Account, and print one line per
// document: the re-encoded JSON, or REJECT for a document the SDK refuses. The
// conformance harness pipes the same documents to every language and asserts
// the lines agree across all of them, so the three have to refuse the same
// malformed input, not just agree on the canonical one.
package main

import (
	"encoding/json"
	"fmt"
	"io"
	"os"
)

const reject = "REJECT"

func main() {
	input, err := io.ReadAll(os.Stdin)
	if err != nil {
		panic(err)
	}
	var documents []json.RawMessage
	if err := json.Unmarshal(input, &documents); err != nil {
		panic(err)
	}
	for _, document := range documents {
		var account Account
		if err := json.Unmarshal(document, &account); err != nil {
			fmt.Println(reject)
			continue
		}
		out, err := json.Marshal(account)
		if err != nil {
			fmt.Println(reject)
			continue
		}
		fmt.Println(string(out))
	}
}
