// Runs the generated SDK against the stand-in library for real: the memo is
// instantiated over the generated Reading type from a literal the SDK builds
// out of the constructor's own arguments, and what Recall answers is that
// Reading.
package main

import (
	"context"
	"fmt"
	"os"

	mathkit "example.com/mathkit/mathkit"
)

func main() {
	want := mathkit.Reading{Value: 2.5, Label: "base"}
	client, err := mathkit.New(want.Value, want.Label)
	if err != nil {
		fmt.Fprintln(os.Stderr, "construction failed:", err)
		os.Exit(1)
	}
	got, err := client.Recall(context.Background())
	if err != nil {
		fmt.Fprintln(os.Stderr, "recall failed:", err)
		os.Exit(1)
	}
	if got != want {
		fmt.Fprintf(os.Stderr, "recall: got %+v, want %+v\n", got, want)
		os.Exit(1)
	}
	fmt.Println("ok")
}
