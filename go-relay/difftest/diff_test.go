package difftest

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"testing"
)

// Run with the Rust driver built (see go-relay/README.md):
//
//	HEARTH_RUST_DRIVER=<repo>/target/debug/hearth_relay_diffdriver DIFF_RUNS=200 DIFF_STEPS=300 go test ./difftest/
//
// Without HEARTH_RUST_DRIVER the tests skip: the Rust side is not built by go test.

func rustSide(t *testing.T) *RustSide {
	t.Helper()
	p := os.Getenv("HEARTH_RUST_DRIVER")
	if p == "" {
		t.Skip("HEARTH_RUST_DRIVER not set")
	}
	r, err := NewRustSide(p, t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { r.Close() })
	return r
}

func envInt(name string, def uint64) uint64 {
	if v, err := strconv.ParseUint(os.Getenv(name), 10, 64); err == nil {
		return v
	}
	return def
}

// TestVectorsAgreeOnBothSides replays relay_v1.json through both sides: the harness's
// own check that it drives both the same way.
func TestVectorsAgreeOnBothSides(t *testing.T) {
	rust := rustSide(t)
	goSide := NewGoSide(t.TempDir())
	defer goSide.Close()
	raw, err := os.ReadFile("../../vectors/relay_v1.json")
	if err != nil {
		t.Fatal(err)
	}
	var v struct {
		Cases []struct {
			Name   string            `json:"name"`
			Config map[string]uint64 `json:"config"`
			Steps  []struct {
				Now      uint64 `json:"now"`
				Method   string `json:"method"`
				Path     string `json:"path"`
				Body     string `json:"body"`
				Status   int    `json:"status"`
				Response string `json:"response"`
				Sweep    bool   `json:"sweep"`
				Restart  bool   `json:"restart"`
			} `json:"steps"`
			StoreDigest string `json:"store_digest"`
		} `json:"cases"`
	}
	if err := json.Unmarshal(raw, &v); err != nil {
		t.Fatal(err)
	}
	for _, c := range v.Cases {
		for _, s := range []Side{goSide, rust} {
			if err := s.Reset(c.Config); err != nil {
				t.Fatal(err)
			}
		}
		var last Answer
		for i, st := range c.Steps {
			step := Step{Method: st.Method, Path: st.Path, Body: st.Body, Now: st.Now, Sweep: st.Sweep, Restart: st.Restart}
			a, err := goSide.Do(step)
			if err != nil {
				t.Fatal(err)
			}
			b, err := rust.Do(step)
			if err != nil {
				t.Fatal(err)
			}
			if a != b || a.Status != st.Status || a.Body != st.Response {
				t.Fatalf("%s step %d: go %+v rust %+v want %d %s", c.Name, i, a, b, st.Status, st.Response)
			}
			last = a
		}
		if last.Digest != c.StoreDigest {
			t.Fatalf("%s: digest %s want %s", c.Name, last.Digest, c.StoreDigest)
		}
	}
}

// Divergence is one run's first disagreement, with everything needed to replay it.
type Divergence struct {
	Seed   uint64            `json:"seed"`
	Config map[string]uint64 `json:"config"`
	Steps  []Step            `json:"steps"` // the last one diverged
	Go     Answer            `json:"go"`
	Rust   Answer            `json:"rust"`
	Error  string            `json:"error,omitempty"`
}

// TestDifferential runs DIFF_RUNS generated sequences of DIFF_STEPS steps through both
// relays and stops each at its first disagreement, which it writes to DIFF_OUT.
func TestDifferential(t *testing.T) {
	rust := rustSide(t)
	goSide := NewGoSide(t.TempDir())
	defer goSide.Close()
	runs, steps, base := envInt("DIFF_RUNS", 20), envInt("DIFF_STEPS", 200), envInt("DIFF_SEED", 1)
	skew := os.Getenv("DIFF_SKEW") == "1"
	out := os.Getenv("DIFF_OUT")
	kinds, seen := map[string]int{}, map[string]int{}
	total, found := 0, 0
	for run := uint64(0); run < runs; run++ {
		seed := base + run
		g := NewGen(seed, skew)
		for _, s := range []Side{goSide, rust} {
			if err := s.Reset(g.Config); err != nil {
				t.Fatal(err)
			}
		}
		var hist []Step
		for i := uint64(0); i < steps; i++ {
			st := g.Next()
			hist = append(hist, st)
			a, errA := goSide.Do(st)
			b, errB := rust.Do(st)
			total++
			if errA == nil && errB == nil && a == b {
				seen[fmt.Sprintf("%-28s %d", st.Note, a.Status)]++
				g.Observe(st, a)
				continue
			}
			found++
			d := Divergence{Seed: seed, Config: g.Config, Steps: hist, Go: a, Rust: b}
			if errA != nil || errB != nil {
				d.Error = fmt.Sprintf("go: %v; rust: %v", errA, errB)
			}
			kind := fmt.Sprintf("%s go=%d rust=%d digest_equal=%v", st.Note, a.Status, b.Status, a.Digest == b.Digest)
			kinds[kind]++
			t.Errorf("seed %d step %d: %s\n  go   %+v\n  rust %+v\n  err  %s", seed, i, kind, a, b, d.Error)
			if out != "" {
				os.MkdirAll(out, 0o755)
				j, _ := json.MarshalIndent(d, "", " ")
				os.WriteFile(filepath.Join(out, fmt.Sprintf("seed-%d.json", seed)), j, 0o644)
			}
			if errB != nil {
				// The driver is gone; restart it for the next run.
				rust.Close()
				nr, err := NewRustSide(os.Getenv("HEARTH_RUST_DRIVER"), t.TempDir())
				if err != nil {
					t.Fatal(err)
				}
				*rust = *nr
			}
			break
		}
	}
	t.Logf("%d runs, %d steps compared, %d divergences", runs, total, found)
	for k, n := range kinds {
		t.Logf("  %4d  %s", n, k)
	}
	// What the agreeing steps answered: evidence the sequences reach the deep checks.
	keys := make([]string, 0, len(seen))
	for k := range seen {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	for _, k := range keys {
		t.Logf("  agreed %6d  %s", seen[k], k)
	}
}
