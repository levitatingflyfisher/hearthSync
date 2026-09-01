// Command godriver is the Go relay's handler behind a line protocol on stdin/stdout,
// the mirror of rustdriver, so the Rust relay's end-to-end tests (relay/tests/e2e.rs,
// kernel replicas syncing through a relay) can run against the Go relay too. Each
// stdin line is one JSON command; each gets one JSON line back:
//
//	{"op":"reset","config":{name: n...},"dir":path} → a fresh relay in dir → {"ok":true}
//	{"op":"req","method":…,"path":…,"body":hex,"now":n} → {"status":n,"body":hex}
//	{"op":"sweep","now":n} → {"pruned":n}
//	{"op":"restart"} → close and reopen over the same store → {"ok":true}
//
// Stores are opened without fsync: tests only.
package main

import (
	"bufio"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"

	"hearthsync/go-relay/internal/relay"
)

type command struct {
	Op     string            `json:"op"`
	Config map[string]uint64 `json:"config"`
	Dir    string            `json:"dir"`
	Method string            `json:"method"`
	Path   string            `json:"path"`
	Body   string            `json:"body"`
	Now    uint64            `json:"now"`
}

func main() {
	in := bufio.NewScanner(os.Stdin)
	in.Buffer(make([]byte, 1<<20), 64<<20)
	out := bufio.NewWriter(os.Stdout)
	var r *relay.Relay
	var dir string
	var cfg relay.Config
	die := func(err error) {
		fmt.Fprintln(os.Stderr, "godriver:", err)
		os.Exit(1)
	}
	for in.Scan() {
		var c command
		if err := json.Unmarshal(in.Bytes(), &c); err != nil {
			die(err)
		}
		var answer any
		switch c.Op {
		case "reset", "restart":
			if c.Op == "reset" {
				dir, cfg = c.Dir, relay.DefaultConfig()
				for k, v := range c.Config {
					if !cfg.Set(k, v) {
						die(fmt.Errorf("unknown config key %s", k))
					}
				}
			}
			if r != nil {
				r.Close()
			}
			var err error
			if r, err = relay.Open(dir, cfg, true); err != nil {
				die(err)
			}
			answer = map[string]any{"ok": true}
		case "req":
			body, err := hex.DecodeString(c.Body)
			if err != nil {
				die(err)
			}
			resp := r.Handle(c.Method, c.Path, body, c.Now)
			answer = map[string]any{"status": resp.Status, "body": hex.EncodeToString(resp.Body)}
		case "sweep":
			n, err := r.Sweep(c.Now)
			if err != nil {
				die(err)
			}
			answer = map[string]any{"pruned": n}
		default:
			die(fmt.Errorf("unknown op %q", c.Op))
		}
		b, _ := json.Marshal(answer)
		out.Write(append(b, '\n'))
		out.Flush()
	}
	if r != nil {
		r.Close()
	}
}
