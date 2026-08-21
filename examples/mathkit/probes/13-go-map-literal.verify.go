// Runs the generated SDK against the stand-in library for real: the map
// literal the binding passes reaches FromTable as a typed Go map, and
// Compute answers the entry keyed "answer".
package main

import (
	"context"
	"fmt"
	"os"

	mathkit "example.com/mathkit/mathkit"
)

func main() {
	client, err := mathkit.New(42)
	if err != nil {
		fmt.Fprintln(os.Stderr, "construction failed:", err)
		os.Exit(1)
	}
	got, err := client.Value(context.Background())
	if err != nil {
		fmt.Fprintln(os.Stderr, "value failed:", err)
		os.Exit(1)
	}
	if got != 42 {
		fmt.Fprintf(os.Stderr, "value: got %v, want 42\n", got)
		os.Exit(1)
	}
	fmt.Println("ok")
}
