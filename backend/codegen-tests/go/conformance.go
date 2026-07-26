//go:build conformance

// The conformance driver: read a batch of wire JSON documents from stdin (one
// per line), decode each into the generated Account via json.Unmarshal,
// re-encode it via json.Marshal, and print one document per line. The harnesses
// pipe the same batch to every language and compare the re-encoded JSON
// Value-wise across all of them.
package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
	"strings"
)

func main() {
	scanner := bufio.NewScanner(os.Stdin)
	scanner.Buffer(make([]byte, 0, 1024*1024), 1024*1024)
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" {
			continue
		}
		var account Account
		if err := json.Unmarshal([]byte(line), &account); err != nil {
			panic(err)
		}
		out, err := json.Marshal(account)
		if err != nil {
			panic(err)
		}
		fmt.Println(string(out))
	}
	if err := scanner.Err(); err != nil {
		panic(err)
	}
}
