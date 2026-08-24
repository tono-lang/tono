// Runs the generated SDK against the stand-in library for real: each client
// builds its own options literal and hands Connect its address, so two
// sessions opened from different arguments answer with what their own
// literal carried (an empty greeting falls back to the library's "pong").
package main

import (
	"context"
	"fmt"
	"os"

	mathkit "example.com/mathkit/mathkit"
)

func main() {
	cases := []struct{ addr, greeting, want string }{
		{"calc.local", "hello", "hello from calc.local"},
		{"calc.remote", "", "pong from calc.remote"},
	}
	for _, c := range cases {
		client, err := mathkit.New(c.addr, c.greeting)
		if err != nil {
			fmt.Fprintf(os.Stderr, "construction for %s failed: %v\n", c.addr, err)
			os.Exit(1)
		}
		got, err := client.Ping(context.Background())
		if err != nil || got != c.want {
			fmt.Fprintf(os.Stderr, "ping %s: got %q, %v; want %q\n", c.addr, got, err, c.want)
			os.Exit(1)
		}
	}
	fmt.Println("ok")
}
