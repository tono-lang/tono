// Runs the generated SDK against the stand-in library for real: the memo is
// instantiated over the generated Reading type, and what Recall answers is
// the Reading the constructor remembered.
package main

import (
	"context"
	"fmt"
	"os"

	mathkit "example.com/mathkit/mathkit"
)

func main() {
	seed := mathkit.Reading{Value: 2.5, Label: "base"}
	client, err := mathkit.New(seed)
	if err != nil {
		fmt.Fprintln(os.Stderr, "construction failed:", err)
		os.Exit(1)
	}
	got, err := client.Recall(context.Background())
	if err != nil {
		fmt.Fprintln(os.Stderr, "recall failed:", err)
		os.Exit(1)
	}
	if got != seed {
		fmt.Fprintf(os.Stderr, "recall: got %+v, want %+v\n", got, seed)
		os.Exit(1)
	}
	fmt.Println("ok")
}
