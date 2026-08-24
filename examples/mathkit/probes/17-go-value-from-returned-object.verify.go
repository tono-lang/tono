// Runs the generated SDK against the stand-in library for real: a key the
// session knows answers through Reading.Result, and a key it does not
// know fails through the same Result, so both the value and the error
// prove they crossed the returned object rather than the call.
package main

import (
	"context"
	"fmt"
	"os"

	mathkit "example.com/mathkit/mathkit"
)

func main() {
	client, err := mathkit.New("calc.local")
	if err != nil {
		fmt.Fprintln(os.Stderr, "construction failed:", err)
		os.Exit(1)
	}
	ctx := context.Background()
	got, err := client.Read(ctx, "addr")
	if err != nil || got != "calc.local" {
		fmt.Fprintf(os.Stderr, "read addr: got %q, %v; want %q\n", got, err, "calc.local")
		os.Exit(1)
	}
	if _, err := client.Read(ctx, "nothing"); err == nil {
		fmt.Fprintln(os.Stderr, "read nothing: expected the error Result carries, got none")
		os.Exit(1)
	}
	fmt.Println("ok")
}
