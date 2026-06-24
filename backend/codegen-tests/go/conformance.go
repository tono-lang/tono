//go:build conformance

// The conformance driver: read a canonical wire JSON from stdin, decode it into
// the generated types via decodeAccount, re-encode it via encodeAccount, and
// print the result. The conformance harness pipes the same fixture to every
// language and asserts the re-encoded JSON is Value-equal across all of them.
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
	var raw any
	if err := json.Unmarshal(input, &raw); err != nil {
		panic(err)
	}
	account, err := decodeAccount(raw)
	if err != nil {
		panic(err)
	}
	out, err := json.Marshal(encodeAccount(account))
	if err != nil {
		panic(err)
	}
	fmt.Println(string(out))
}
