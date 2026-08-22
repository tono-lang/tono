// Runs the generated SDK against the stand-in library for real: two
// already-built handles are spread into FromFallback's variadic slot, and
// the "first" strategy answers the primary one's value.
package main

import (
	"context"
	"fmt"
	"os"

	mathkit "example.com/mathkit/mathkit"
)

func main() {
	client, err := mathkit.New(1.5, 9.0)
	if err != nil {
		fmt.Fprintln(os.Stderr, "construction failed:", err)
		os.Exit(1)
	}
	value, err := client.Value(context.Background())
	if err != nil {
		fmt.Fprintln(os.Stderr, "value failed:", err)
		os.Exit(1)
	}
	if value != 1.5 {
		fmt.Fprintf(os.Stderr, "value: got %v, want 1.5\n", value)
		os.Exit(1)
	}
	fmt.Println("ok")
}
