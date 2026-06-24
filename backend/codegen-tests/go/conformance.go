//go:build conformance

// The conformance driver: read a canonical wire JSON from stdin, decode it into
// the generated types, re-encode it, and print the result. The conformance
// harness pipes the same fixture to every language and asserts the re-encoded
// JSON is Value-equal across all of them.
package main

import (
	"encoding/json"
	"fmt"
	"io"
	"os"
)

func main() {
	input, err := io.ReadAll(os.Stdin)
	if err != nil {
		panic(err)
	}
	var account Account
	if err := json.Unmarshal(input, &account); err != nil {
		panic(err)
	}
	out, err := json.Marshal(account)
	if err != nil {
		panic(err)
	}
	fmt.Println(string(out))
}
