// Runs the generated SDK against the stand-in library for real: the client
// holds the interface value FromConstant returned and reads through it.
package main

import (
	"context"
	"fmt"
	"os"

	mathkit "example.com/mathkit/mathkit"
)

func main() {
	client, err := mathkit.New(2.5)
	if err != nil {
		fmt.Fprintln(os.Stderr, "construction failed:", err)
		os.Exit(1)
	}
	got, err := client.Value(context.Background())
	if err != nil {
		fmt.Fprintln(os.Stderr, "value failed:", err)
		os.Exit(1)
	}
	if got != 2.5 {
		fmt.Fprintf(os.Stderr, "value: got %v, want 2.5\n", got)
		os.Exit(1)
	}
	fmt.Println("ok")
}
