// Runs the generated SDK against the stand-in library for real: the session
// Dial returned (one value, no error) and the one Open returned (with the
// error) both answer Ping through the same generated client, so the two
// return conventions are proven side by side.
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
	for name, ping := range map[string]func(context.Context) (string, error){
		"dial": client.PingDirect,
		"open": client.PingChecked,
	} {
		got, err := ping(ctx)
		if err != nil {
			fmt.Fprintf(os.Stderr, "ping through %s failed: %v\n", name, err)
			os.Exit(1)
		}
		if got != "pong from calc.local" {
			fmt.Fprintf(os.Stderr, "ping through %s: got %q, want %q\n", name, got, "pong from calc.local")
			os.Exit(1)
		}
	}
	fmt.Println("ok")
}
