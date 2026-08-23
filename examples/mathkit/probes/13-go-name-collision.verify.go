// Runs the generated SDK against the stand-in library for real: the client
// holds the library's *Client (not its own generated Client) and reads
// through it.
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
	got, err := client.Ping(context.Background())
	if err != nil {
		fmt.Fprintln(os.Stderr, "ping failed:", err)
		os.Exit(1)
	}
	if got != "pong from calc.local" {
		fmt.Fprintf(os.Stderr, "ping: got %q, want %q\n", got, "pong from calc.local")
		os.Exit(1)
	}
	fmt.Println("ok")
}
