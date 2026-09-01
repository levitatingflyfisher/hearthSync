package relay

import (
	"encoding/hex"
	"encoding/json"
	"os"
	"testing"
)

// The shared conformance suite: every case in vectors/relay_v1.json, replayed against
// the handler, must answer each step byte for byte and leave the store with the case's
// digest. The Rust relay runs the same file (relay/tests/conformance.rs).

type vectorStep struct {
	Now      uint64 `json:"now"`
	Method   string `json:"method"`
	Path     string `json:"path"`
	Body     string `json:"body"`
	Status   int    `json:"status"`
	Response string `json:"response"`
	Expect   string `json:"expect"`
	Sweep    bool   `json:"sweep"`
	Restart  bool   `json:"restart"`
}

type vectorCase struct {
	Name        string            `json:"name"`
	Config      map[string]uint64 `json:"config"`
	Steps       []vectorStep      `json:"steps"`
	StoreDigest string            `json:"store_digest"`
}

type vectorFile struct {
	Format string            `json:"format"`
	Config map[string]uint64 `json:"config"`
	Cases  []vectorCase      `json:"cases"`
}

func loadVectors(t *testing.T) vectorFile {
	t.Helper()
	raw, err := os.ReadFile("../../../vectors/relay_v1.json")
	if err != nil {
		t.Fatal(err)
	}
	var v vectorFile
	if err := json.Unmarshal(raw, &v); err != nil {
		t.Fatal(err)
	}
	return v
}

func configWith(t *testing.T, base Config, over map[string]uint64) Config {
	t.Helper()
	for k, n := range over {
		if !base.Set(k, n) {
			t.Fatalf("unknown config key %s", k)
		}
	}
	return base
}

func unhex(t *testing.T, s string) []byte {
	t.Helper()
	b, err := hex.DecodeString(s)
	if err != nil {
		t.Fatal(err)
	}
	return b
}

func TestRelayV1Vectors(t *testing.T) {
	v := loadVectors(t)
	if v.Format != "hearthSync relay conformance v1" {
		t.Fatalf("format %q", v.Format)
	}
	base := configWith(t, DefaultConfig(), v.Config)
	if base != DefaultConfig() {
		t.Fatalf("the vectors' defaults are not the protocol's: %+v", base)
	}
	if len(v.Cases) < 10 {
		t.Fatalf("%d cases", len(v.Cases))
	}
	steps := 0
	for _, c := range v.Cases {
		t.Run(c.Name, func(t *testing.T) {
			dir, cfg := t.TempDir(), configWith(t, base, c.Config)
			r, err := Open(dir, cfg, true)
			if err != nil {
				t.Fatal(err)
			}
			defer func() { r.Close() }()
			for i, s := range c.Steps {
				if s.Sweep {
					if _, err := r.Sweep(s.Now); err != nil {
						t.Fatal(err)
					}
					continue
				}
				if s.Restart {
					// Stop and reopen over the same store: memory is lost.
					r.Close()
					if r, err = Open(dir, cfg, true); err != nil {
						t.Fatal(err)
					}
					continue
				}
				got := r.Handle(s.Method, s.Path, unhex(t, s.Body), s.Now)
				if got.Status != s.Status || hex.EncodeToString(got.Body) != s.Response {
					t.Fatalf("step %d (expects %s): got %d %x, want %d %s", i, s.Expect, got.Status, got.Body, s.Status, s.Response)
				}
				steps++
			}
			d, err := r.Digest()
			if err != nil {
				t.Fatal(err)
			}
			if hex.EncodeToString(d[:]) != c.StoreDigest {
				t.Fatalf("store digest %x, want %s", d, c.StoreDigest)
			}
		})
	}
	if steps <= 100 {
		t.Fatalf("%d steps", steps)
	}
}
