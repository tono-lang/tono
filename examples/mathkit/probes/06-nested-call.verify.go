// Runs the generated SDK against the stand-in library for real: the
// precision option is built by a nested call, WithPrecision(precision),
// baked directly into FromFormula's own call: line.
package main

import (
	"context"
	"fmt"
	"os"

	mathkit "example.com/mathkit/mathkit"
)

func main() {
	client, err := mathkit.New("22 / 7")
	if err != nil {
		fmt.Fprintln(os.Stderr, "construction failed:", err)
		os.Exit(1)
	}
	first, err := client.Value(context.Background())
	if err != nil {
		fmt.Fprintln(os.Stderr, "value failed:", err)
		os.Exit(1)
	}
	if first != 3.1429 {
		fmt.Fprintf(os.Stderr, "value: got %v, want 3.1429\n", first)
		os.Exit(1)
	}
	// compute() is idempotent: reading the already-built formula twice
	// answers the same value both times.
	second, err := client.Value(context.Background())
	if err != nil {
		fmt.Fprintln(os.Stderr, "second value failed:", err)
		os.Exit(1)
	}
	if second != first {
		fmt.Fprintf(os.Stderr, "value: got %v on the second call, want %v\n", second, first)
		os.Exit(1)
	}
	fmt.Println("ok")
}
