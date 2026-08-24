// Runs the generated SDK against the stand-in library for real: the int64
// the client takes crosses WithPrecision's int parameter converted, and the
// rounded result proves the option was applied.
package main

import (
	"context"
	"fmt"
	"os"

	mathkit "example.com/mathkit/mathkit"
)

func main() {
	client, err := mathkit.New("22 / 7", 2)
	if err != nil {
		fmt.Fprintln(os.Stderr, "construction failed:", err)
		os.Exit(1)
	}
	got, err := client.Value(context.Background())
	if err != nil {
		fmt.Fprintln(os.Stderr, "value failed:", err)
		os.Exit(1)
	}
	if got != 3.14 {
		fmt.Fprintf(os.Stderr, "value: got %v, want 3.14\n", got)
		os.Exit(1)
	}
	fmt.Println("ok")
}
