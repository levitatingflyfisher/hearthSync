package relay

import (
	"bufio"
	"bytes"
	"context"
	"crypto/ed25519"
	"fmt"
	"io"
	"net"
	"strconv"
	"strings"
	"testing"
	"time"

	"hearthsync/go-relay/internal/dcbor"
)

// The HTTP layer: CORS, body limits, content types, and the handler over real HTTP. The
// vectors drive the handler only, so this is the transport's own test (the Rust
// relay's relay/tests/http.rs, ported).

type reply struct {
	status  int
	headers map[string]string
	body    []byte
}

// request sends raw bytes over a fresh connection and reads the whole answer (no HTTP
// client, so nothing normalises what is sent).
func request(t *testing.T, addr string, raw []byte) reply {
	t.Helper()
	c, err := net.Dial("tcp", addr)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	c.SetDeadline(time.Now().Add(10 * time.Second))
	go func() { c.Write(raw) }()
	br := bufio.NewReader(c)
	line, err := br.ReadString('\n')
	if err != nil {
		t.Fatal(err)
	}
	status, _ := strconv.Atoi(strings.Fields(line)[1])
	r := reply{status: status, headers: map[string]string{}}
	for {
		l, err := br.ReadString('\n')
		if err != nil {
			t.Fatal(err)
		}
		l = strings.TrimRight(l, "\r\n")
		if l == "" {
			break
		}
		k, v, _ := strings.Cut(l, ":")
		r.headers[strings.ToLower(k)] = strings.TrimSpace(v)
	}
	r.body, _ = io.ReadAll(br)
	return r
}

func post(t *testing.T, addr, path string, body []byte) reply {
	raw := fmt.Sprintf("POST %s HTTP/1.1\r\nHost: relay\r\nContent-Type: application/cbor\r\nContent-Length: %d\r\nConnection: close\r\n\r\n", path, len(body))
	return request(t, addr, append([]byte(raw), body...))
}

func withServer(t *testing.T, cfg Config, f func(addr string)) {
	t.Helper()
	withServerLimits(t, cfg, LimitsFor(cfg), f)
}

func withServerLimits(t *testing.T, cfg Config, lim HTTPLimits, f func(addr string)) {
	t.Helper()
	r, err := Open(t.TempDir(), cfg, true)
	if err != nil {
		t.Fatal(err)
	}
	var logs bytes.Buffer
	SetLogOutput(&logs)
	defer SetLogOutput(nil)
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	srv := NewServer(r, lim)
	done := make(chan error, 1)
	go func() { done <- srv.Serve(ctx, ln) }()
	f(ln.Addr().String())
	cancel()
	if err := <-done; err != nil {
		t.Fatal(err)
	}
	srv.Close()
}

const zeroCH = "0000000000000000000000000000000000000000000000000000000000000000"

func TestCORSPreflightAndEveryAnswerAllowAnyOrigin(t *testing.T) {
	withServer(t, DefaultConfig(), func(addr string) {
		r := request(t, addr, []byte("OPTIONS /v1/"+zeroCH+"/pull HTTP/1.1\r\nHost: relay\r\nOrigin: https://app.example\r\nAccess-Control-Request-Method: POST\r\nConnection: close\r\n\r\n"))
		if r.status != 204 || r.headers["access-control-allow-origin"] != "*" ||
			r.headers["access-control-allow-methods"] != "POST" ||
			r.headers["access-control-allow-headers"] != "content-type" ||
			r.headers["access-control-max-age"] != "86400" {
			t.Fatalf("preflight: %+v", r)
		}
		r = post(t, addr, "/v1/"+zeroCH+"/pull", []byte{0x80})
		if r.status != 400 || r.headers["access-control-allow-origin"] != "*" ||
			r.headers["content-type"] != "application/cbor" || string(r.body) != "\x82\x63err\x6bbad_request" {
			t.Fatalf("bad request: %+v", r)
		}
		r = request(t, addr, []byte("GET /healthz HTTP/1.1\r\nHost: relay\r\nConnection: close\r\n\r\n"))
		if r.status != 200 || string(r.body) != "ok" {
			t.Fatalf("healthz: %+v", r)
		}
		// Paths are routed as sent: never percent-decoded, never cleaned.
		for _, p := range []string{"/v1/%30" + zeroCH[1:] + "/pull", "/v1/" + zeroCH + "/../" + zeroCH + "/pull", "//v1/" + zeroCH + "/pull"} {
			if r := post(t, addr, p, []byte{0x80}); r.status != 404 {
				t.Fatalf("%s: %d", p, r.status)
			}
		}
		if r := post(t, addr, "/healthz", nil); r.status != 404 {
			t.Fatalf("POST /healthz: %d", r.status)
		}
	})
}

func TestBodiesOverTheLimitAreRefusedDeclaredOrStreamed(t *testing.T) {
	cfg := DefaultConfig()
	cfg.MaxSnapshot, cfg.MaxBatch, cfg.MaxEnvelope = 1000, 1, 100
	limit := cfg.MaxBody()
	withServer(t, cfg, func(addr string) {
		raw := fmt.Sprintf("POST /v1/%s/append HTTP/1.1\r\nHost: relay\r\nContent-Length: %d\r\nConnection: close\r\n\r\n", zeroCH, limit+1)
		if r := request(t, addr, []byte(raw)); r.status != 413 {
			t.Fatalf("declared: %d", r.status)
		}
		var b bytes.Buffer
		fmt.Fprintf(&b, "POST /v1/%s/append HTTP/1.1\r\nHost: relay\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n", zeroCH)
		chunk := bytes.Repeat([]byte{0x41}, 4096)
		for i := uint64(0); i < limit/4096+2; i++ {
			fmt.Fprintf(&b, "%x\r\n", len(chunk))
			b.Write(chunk)
			b.WriteString("\r\n")
		}
		b.WriteString("0\r\n\r\n")
		r := request(t, addr, b.Bytes())
		if r.status != 413 || string(r.body) != "\x82\x63err\x69too_large" {
			t.Fatalf("streamed: %+v", r)
		}
	})
}

func TestTheHandlerAnswersTheSameOverHTTP(t *testing.T) {
	hk := ed25519.NewKeyFromSeed(bytes.Repeat([]byte{3}, 32))
	dk := ed25519.NewKeyFromSeed(bytes.Repeat([]byte{4}, 32))
	var hh, dev ID
	copy(hh[:], hk.Public().(ed25519.PublicKey))
	copy(dev[:], dk.Public().(ed25519.PublicKey))
	ch := ChannelID("lullaby", &hh)
	chHex := fmt.Sprintf("%x", ch)
	withServer(t, DefaultConfig(), func(addr string) {
		auth := ed25519.Sign(hk, enrollAuthMsg("lullaby", &dev, "phone"))
		body := dcbor.Enc{}.Array(5).Text("lullaby").Bytes(hh[:]).Bytes(dev[:]).Text("phone").Bytes(auth)
		if r := post(t, addr, "/v1/"+chHex+"/enroll", body); r.status != 200 || string(r.body) != "\x82\x62ok\x01" {
			t.Fatalf("enroll: %+v", r)
		}
		q := pullReq{reader: dev, ts: NowMS(), epoch: 1, nonce: [16]byte{1}}
		copy(q.sig[:], ed25519.Sign(dk, q.signable(&ch)))
		body = dcbor.Enc{}.Array(6).Bytes(dev[:]).Uint(q.ts).Uint(q.epoch).Bytes(q.nonce[:]).Array(0).Bytes(q.sig[:])
		r := post(t, addr, "/v1/"+chHex+"/pull", body)
		if r.status != 200 || !bytes.HasPrefix(r.body, []byte("\x85\x62ok\x01")) {
			t.Fatalf("pull: %+v", r)
		}
		r = request(t, addr, []byte("GET /v1/"+chHex+"/pull HTTP/1.1\r\nHost: relay\r\nConnection: close\r\n\r\n"))
		if r.status != 405 {
			t.Fatalf("GET: %d", r.status)
		}
	})
}

func TestTheTestSweepHookExpiresIdleChannelsAheadOfTimeOnlyWhenEnabled(t *testing.T) {
	hk := ed25519.NewKeyFromSeed(bytes.Repeat([]byte{3}, 32))
	dk := ed25519.NewKeyFromSeed(bytes.Repeat([]byte{4}, 32))
	var hh, dev ID
	copy(hh[:], hk.Public().(ed25519.PublicKey))
	copy(dev[:], dk.Public().(ed25519.PublicKey))
	ch := ChannelID("lullaby", &hh)
	chHex := fmt.Sprintf("%x", ch)
	auth := ed25519.Sign(hk, enrollAuthMsg("lullaby", &dev, "phone"))
	enroll := dcbor.Enc{}.Array(5).Text("lullaby").Bytes(hh[:]).Bytes(dev[:]).Text("phone").Bytes(auth)
	cfg := DefaultConfig()
	cfg.IdleMS = 10000
	// Off by default: the path is unknown, like any other.
	withServer(t, cfg, func(addr string) {
		if r := post(t, addr, "/test/sweep", []byte("20000")); r.status != 404 {
			t.Fatalf("hook without the flag: %d", r.status)
		}
	})
	lim := LimitsFor(cfg)
	lim.TestHooks = true
	withServerLimits(t, cfg, lim, func(addr string) {
		gen := func() string { return string(post(t, addr, "/v1/"+chHex+"/enroll", enroll).body) }
		if g := gen(); g != "\x82\x62ok\x01" {
			t.Fatalf("enroll: %q", g)
		}
		// A sweep as of now: the channel is not idle yet.
		if r := post(t, addr, "/test/sweep", nil); r.status != 200 || string(r.body) != "ok" {
			t.Fatalf("sweep now: %+v", r)
		}
		if g := gen(); g != "\x82\x62ok\x01" {
			t.Fatalf("after a sweep now: %q", g)
		}
		// As of 20 s from now: it expired, so enrolling again makes it anew.
		if r := post(t, addr, "/test/sweep", []byte("20000")); r.status != 200 {
			t.Fatalf("sweep ahead: %+v", r)
		}
		if g := gen(); g != "\x82\x62ok\x02" {
			t.Fatalf("after a sweep ahead: %q", g)
		}
		// The relay's clock did not move: a read signed now is fresh, not stale.
		q := pullReq{reader: dev, ts: NowMS(), epoch: 1, nonce: [16]byte{1}}
		copy(q.sig[:], ed25519.Sign(dk, q.signable(&ch)))
		body := dcbor.Enc{}.Array(6).Bytes(dev[:]).Uint(q.ts).Uint(q.epoch).Bytes(q.nonce[:]).Array(0).Bytes(q.sig[:])
		if r := post(t, addr, "/v1/"+chHex+"/pull", body); r.status != 200 {
			t.Fatalf("pull after the hook: %+v", r)
		}
		if r := post(t, addr, "/test/sweep", []byte("soon")); r.status != 400 {
			t.Fatalf("bad body: %d", r.status)
		}
		if r := request(t, addr, []byte("GET /test/sweep HTTP/1.1\r\nHost: relay\r\nConnection: close\r\n\r\n")); r.status != 404 {
			t.Fatalf("GET: %d", r.status)
		}
	})
}

func TestConnectionsPastTheCapAreClosed(t *testing.T) {
	cfg := DefaultConfig()
	r, err := Open(t.TempDir(), cfg, true)
	if err != nil {
		t.Fatal(err)
	}
	SetLogOutput(io.Discard)
	defer SetLogOutput(nil)
	ln, _ := net.Listen("tcp", "127.0.0.1:0")
	lim := LimitsFor(cfg)
	lim.MaxConnections = 1
	lim.ConnTimeout = 500 * time.Millisecond
	ctx, cancel := context.WithCancel(context.Background())
	srv := NewServer(r, lim)
	done := make(chan error, 1)
	go func() { done <- srv.Serve(ctx, ln) }()
	addr := ln.Addr().String()
	first, _ := net.Dial("tcp", addr) // holds the only slot, idle
	time.Sleep(100 * time.Millisecond)
	second, _ := net.Dial("tcp", addr)
	second.SetReadDeadline(time.Now().Add(2 * time.Second))
	if _, err := second.Read(make([]byte, 1)); err != io.EOF {
		t.Fatalf("second connection not closed: %v", err)
	}
	// The first ends at its lifetime, and frees the slot.
	first.SetReadDeadline(time.Now().Add(3 * time.Second))
	if _, err := first.Read(make([]byte, 1)); err != io.EOF {
		t.Fatalf("first connection outlived its lifetime: %v", err)
	}
	if r := request(t, addr, []byte("GET /healthz HTTP/1.1\r\nHost: relay\r\nConnection: close\r\n\r\n")); r.status != 200 {
		t.Fatalf("slot not freed: %d", r.status)
	}
	cancel()
	<-done
	srv.Close()
}
