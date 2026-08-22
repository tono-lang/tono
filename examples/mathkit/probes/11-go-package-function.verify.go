// Runs the generated SDK against the stand-in library for real: the
// constructor that is a static method elsewhere is the package function
// FromFormula here, written as the Go block's own callee.
package main

import (
	"context"
	"fmt"
	"os"

	mathkit "example.com/mathkit/mathkit"
)

func main() {
	client, err := mathkit.New("2 * 3")
	if err != nil {
		fmt.Fprintln(os.Stderr, "construction failed:", err)
		os.Exit(1)
	}
	value, err := client.Value(context.Background())
	if err != nil {
		fmt.Fprintln(os.Stderr, "value failed:", err)
		os.Exit(1)
	}
	if value != 6 {
		fmt.Fprintf(os.Stderr, "value: got %v, want 6\n", value)
		os.Exit(1)
	}
	fmt.Println("ok")
}
