//go:build !conformance

// The round-trip driver. The generated types and codecs live in models.go
// (package main), written by the harness at run time. Struct fields carry
// encoding/json tags, so json.Marshal/json.Unmarshal do the wire work directly;
// only the union and the @entries map have generated marshaling. This driver
// asserts the hard wire cases hold: a 64-bit integer above 2^53 travels as a JSON
// string, bytes travel as base64, an internally-tagged union carries its
// discriminator, the open enum is a named string that holds an unknown value, and
// a decode/re-encode is a canonical (Value-equal) round-trip.
package main

import (
	"encoding/json"
	"fmt"
	"os"
	"reflect"
)

func fail(msg string) {
	fmt.Println("FAIL:", msg)
	os.Exit(1)
}

func main() {
	tip := int64(500)
	acct := Account{
		AccountID: 9007199254740993, // 2^53 + 1
		Secret:    []byte{1, 2, 3, 254},
		Tip:       &tip,
		Status:    StatusActive,
		Method:    MethodCard{Value: CardData{Last4: "4242"}},
		Counts:    Entries[int32, string]{{Key: 7, Value: "a"}, {Key: 3, Value: "b"}},
	}

	wire, err := json.Marshal(acct)
	if err != nil {
		panic(err)
	}
	fmt.Println(string(wire))

	var m map[string]any
	if err := json.Unmarshal(wire, &m); err != nil {
		panic(err)
	}
	if m["account_id"] != "9007199254740993" {
		fail("i64 must encode as a JSON string")
	}
	if _, ok := m["secret"].(string); !ok {
		fail("bytes must encode as a base64 string")
	}
	method, ok := m["method"].(map[string]any)
	if !ok || method["type"] != "card" {
		fail("union must carry its discriminator")
	}
	if counts, ok := m["counts"].([]any); !ok || len(counts) != 2 {
		fail("@entries map must encode as an array of pairs")
	}

	// Canonical round-trip: decode then re-encode must be Value-equal.
	var back Account
	if err := json.Unmarshal(wire, &back); err != nil {
		panic(err)
	}
	again, err := json.Marshal(back)
	if err != nil {
		panic(err)
	}
	var a, b any
	_ = json.Unmarshal(wire, &a)
	_ = json.Unmarshal(again, &b)
	if !reflect.DeepEqual(a, b) {
		fail("round-trip changed the JSON: " + string(again))
	}

	// An open enum is a named string, so an unknown value passes through as-is.
	status := Status("frozen")
	if status != "frozen" {
		fail("an unknown enum value must pass through")
	}

	fmt.Println("ROUNDTRIP_OK")
}
