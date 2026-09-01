// Package difftest drives the Go relay (in process) and the Rust relay (through
// rustdriver, a line protocol over a pipe) with the same request sequences and compares
// every answer and the store digest after every step. Time is an input, so the
// sequences reach staleness, rate limits and pruning deterministically.
package difftest

import (
	"bufio"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"

	"hearthsync/go-relay/internal/relay"
)

// Answer is one side's answer to a step.
type Answer struct {
	Status int    `json:"status"`
	Body   string `json:"body"` // hex
	Digest string `json:"digest"`
}

// Step is one request, a sweep, or a restart (the relay stopped and started again over
// the same store: memory is lost and the epoch moves on).
type Step struct {
	Sweep   bool   `json:"sweep,omitempty"`
	Restart bool   `json:"restart,omitempty"`
	Method  string `json:"method,omitempty"`
	Path    string `json:"path,omitempty"`
	Body    string `json:"body,omitempty"` // hex
	Now     uint64 `json:"now"`
	Note    string `json:"note,omitempty"` // what the generator meant
}

// Side is a relay under test.
type Side interface {
	Reset(cfg map[string]uint64) error
	Do(s Step) (Answer, error)
	Close() error
}

// GoSide is the Go relay, in process, over a bbolt file in dir.
type GoSide struct {
	dir string
	n   int
	cfg relay.Config
	r   *relay.Relay
}

func NewGoSide(dir string) *GoSide { return &GoSide{dir: dir} }

func (g *GoSide) Reset(cfg map[string]uint64) error {
	if g.r != nil {
		g.r.Close()
		os.RemoveAll(filepath.Join(g.dir, fmt.Sprint(g.n)))
	}
	g.n++
	c := relay.DefaultConfig()
	for k, v := range cfg {
		if !c.Set(k, v) {
			return fmt.Errorf("unknown config key %s", k)
		}
	}
	d := filepath.Join(g.dir, fmt.Sprint(g.n))
	if err := os.MkdirAll(d, 0o700); err != nil {
		return err
	}
	r, err := relay.Open(d, c, true)
	g.r, g.cfg = r, c
	return err
}

func (g *GoSide) Do(s Step) (Answer, error) {
	var a Answer
	switch {
	case s.Restart:
		g.r.Close()
		r, err := relay.Open(filepath.Join(g.dir, fmt.Sprint(g.n)), g.cfg, true)
		if err != nil {
			return a, err
		}
		g.r = r
	case s.Sweep:
		if _, err := g.r.Sweep(s.Now); err != nil {
			return a, err
		}
	default:
		body, err := hex.DecodeString(s.Body)
		if err != nil {
			return a, err
		}
		resp := g.r.Handle(s.Method, s.Path, body, s.Now)
		a.Status, a.Body = resp.Status, hex.EncodeToString(resp.Body)
	}
	d, err := g.r.Digest()
	a.Digest = hex.EncodeToString(d[:])
	return a, err
}

func (g *GoSide) Close() error {
	if g.r != nil {
		return g.r.Close()
	}
	return nil
}

// RustSide is the Rust relay's handler behind rustdriver, over SQLite files in dir.
type RustSide struct {
	cmd *exec.Cmd
	in  io.WriteCloser
	out *bufio.Reader
	dir string
	n   int
}

func NewRustSide(driver, dir string) (*RustSide, error) {
	c := exec.Command(driver)
	c.Stderr = os.Stderr
	in, err := c.StdinPipe()
	if err != nil {
		return nil, err
	}
	out, err := c.StdoutPipe()
	if err != nil {
		return nil, err
	}
	if err := c.Start(); err != nil {
		return nil, err
	}
	return &RustSide{cmd: c, in: in, out: bufio.NewReaderSize(out, 1<<20), dir: dir}, nil
}

func (r *RustSide) call(cmd map[string]any, into any) error {
	b, _ := json.Marshal(cmd)
	if _, err := r.in.Write(append(b, '\n')); err != nil {
		return fmt.Errorf("rust driver: %w", err)
	}
	line, err := r.out.ReadBytes('\n')
	if err != nil {
		return fmt.Errorf("rust driver died: %w", err)
	}
	return json.Unmarshal(line, into)
}

func (r *RustSide) Reset(cfg map[string]uint64) error {
	r.n++
	d := filepath.Join(r.dir, fmt.Sprint(r.n))
	os.RemoveAll(filepath.Join(r.dir, fmt.Sprint(r.n-1)))
	if err := os.MkdirAll(d, 0o700); err != nil {
		return err
	}
	var v map[string]any
	return r.call(map[string]any{"op": "reset", "config": cfg, "dir": d}, &v)
}

func (r *RustSide) Do(s Step) (Answer, error) {
	var a Answer
	if s.Restart {
		err := r.call(map[string]any{"op": "restart"}, &a)
		return a, err
	}
	if s.Sweep {
		err := r.call(map[string]any{"op": "sweep", "now": s.Now}, &a)
		return a, err
	}
	err := r.call(map[string]any{"op": "req", "method": s.Method, "path": s.Path, "body": s.Body, "now": s.Now}, &a)
	return a, err
}

func (r *RustSide) Close() error {
	r.in.Close()
	return r.cmd.Wait()
}
