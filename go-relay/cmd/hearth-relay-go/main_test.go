package main

import (
	"bufio"
	"bytes"
	"fmt"
	"io"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"

	"hearthsync/go-relay/internal/relay"
)

// The CLI's house rules: help and version on stdout, errors on stderr, exit 2 usage /
// 1 failure, nothing on stdout while serving, JSON logs on stderr without ids, a clean
// stop on SIGTERM. The test binary re-runs itself as the CLI.

func TestMain(m *testing.M) {
	if os.Getenv("HEARTH_RELAY_GO_AS_CLI") == "1" {
		os.Exit(run(os.Args[1:]))
	}
	os.Exit(m.Run())
}

func cli(args ...string) *exec.Cmd {
	c := exec.Command(os.Args[0], args...)
	c.Env = append(os.Environ(), "HEARTH_RELAY_GO_AS_CLI=1")
	return c
}

func runCLI(t *testing.T, args ...string) (int, string, string) {
	t.Helper()
	var out, errb bytes.Buffer
	c := cli(args...)
	c.Stdout, c.Stderr = &out, &errb
	err := c.Run()
	code := 0
	if ee, ok := err.(*exec.ExitError); ok {
		code = ee.ExitCode()
	} else if err != nil {
		t.Fatal(err)
	}
	return code, out.String(), errb.String()
}

func TestHelpAndVersionGoToStdoutAndUsageErrorsExit2(t *testing.T) {
	for _, flag := range []string{"-h", "--help"} {
		code, out, errs := runCLI(t, flag)
		if code != 0 || errs != "" || !strings.HasPrefix(out, "hearth-relay-go:") ||
			!strings.Contains(out, "--data <DIR>") || !strings.Contains(out, "--version") || !strings.Contains(out, "--idle-days <N>") {
			t.Fatalf("%s: %d %q %q", flag, code, out, errs)
		}
	}
	if code, out, _ := runCLI(t, "--version"); code != 0 || out != "hearth-relay-go "+version+"\n" {
		t.Fatalf("--version: %d %q", code, out)
	}
	for _, args := range [][]string{
		{}, {"--bogus"}, {"--data"},
		{"--data", ".", "--listen", "nowhere"},
		{"--data", ".", "--listen", "localhost:8080"}, // an IP:port, as the Rust relay's SocketAddr
		{"--data", ".", "--max-channels", "x"},
		{"--data", ".", "--idle-days", "x"},
		{"--data", ".", "--idle-days", "213503982334602"},
		{"--data", ".", "--max-devices", "x"},
		{"--data", ".", "--max-reader-nonces", "-1"},
		{"--data", ".", "--channel-burst", ""},
		{"--data", ".", "--channel-interval-ms", "1.5"},
	} {
		code, out, errs := runCLI(t, args...)
		if code != 2 || out != "" || !strings.HasPrefix(errs, "hearth-relay-go: ") || !strings.Contains(errs, "--help") {
			t.Fatalf("%v: %d %q %q", args, code, out, errs)
		}
	}
}

func TestIdleDaysSetsTheIdlePeriod(t *testing.T) {
	a, err := parse([]string{"--data", ".", "--idle-days", "30"})
	if err != nil || a.cfg.IdleMS != 30*relay.DayMS {
		t.Fatalf("%v %+v", err, a)
	}
	if a, _ := parse([]string{"--data", "."}); a.cfg.IdleMS != 400*relay.DayMS {
		t.Fatalf("default idle: %d", a.cfg.IdleMS)
	}
}

func TestMemoryFlagsSetTheirLimits(t *testing.T) {
	a, err := parse([]string{"--data", ".", "--max-devices", "8", "--max-channels", "50",
		"--max-reader-nonces", "4", "--channel-burst", "30", "--channel-interval-ms", "250"})
	if err != nil {
		t.Fatal(err)
	}
	c := a.cfg
	if c.MaxDevices != 8 || c.MaxChannels != 50 || c.MaxReaderNonces != 4 || c.ChannelBurst != 30 || c.ChannelIntervalMS != 250 {
		t.Fatalf("%+v", c)
	}
}

func TestFailuresExit1(t *testing.T) {
	code, out, errs := runCLI(t, "--data", "/nonexistent/hearth-relay-go-test")
	if code != 1 || out != "" || !strings.Contains(errs, "not a directory") {
		t.Fatalf("%d %q %q", code, out, errs)
	}
}

func TestServingPrintsOnlyJSONLogsAndStopsCleanlyOnSIGTERM(t *testing.T) {
	dir := t.TempDir()
	c := cli("--data", dir, "--listen", "127.0.0.1:0")
	var out bytes.Buffer
	c.Stdout = &out
	errp, _ := c.StderrPipe()
	if err := c.Start(); err != nil {
		t.Fatal(err)
	}
	br := bufio.NewReader(errp)
	first, err := br.ReadString('\n')
	if err != nil || !strings.HasPrefix(first, `{"ts":`) || !strings.Contains(first, `"event":"start"`) {
		t.Fatalf("first log line %q (%v)", first, err)
	}
	addr := strings.SplitN(strings.SplitN(first, `"listen":"`, 2)[1], `"`, 2)[0]
	const ch = "0000000000000000000000000000000000000000000000000000000000000000"
	for _, raw := range []string{
		"GET /healthz HTTP/1.1\r\nHost: relay\r\nConnection: close\r\n\r\n",
		fmt.Sprintf("POST /v1/%s/pull HTTP/1.1\r\nHost: relay\r\nContent-Length: 1\r\nConnection: close\r\n\r\n\x80", ch),
	} {
		conn, err := net.Dial("tcp", addr)
		if err != nil {
			t.Fatal(err)
		}
		conn.SetDeadline(time.Now().Add(10 * time.Second))
		conn.Write([]byte(raw))
		resp, _ := io.ReadAll(conn)
		conn.Close()
		if !bytes.HasPrefix(resp, []byte("HTTP/1.1 200")) && !bytes.HasPrefix(resp, []byte("HTTP/1.1 400")) {
			t.Fatalf("answer %q", resp)
		}
	}
	c.Process.Signal(syscall.SIGTERM)
	rest, _ := io.ReadAll(br)
	if err := c.Wait(); err != nil {
		t.Fatalf("exit: %v", err)
	}
	if out.Len() != 0 {
		t.Fatalf("stdout while serving: %q", out.String())
	}
	for _, line := range strings.Split(strings.TrimSpace(string(rest)), "\n") {
		if !strings.HasPrefix(line, `{"ts":`) || !strings.HasSuffix(line, "}") {
			t.Fatalf("not a JSON log line: %q", line)
		}
		if strings.Contains(line, ch) || strings.Contains(line, "127.0.0.1:") && !strings.Contains(line, `"event":"start"`) {
			t.Fatalf("a log line carries an id or an address: %q", line)
		}
	}
	if !strings.Contains(string(rest), `"event":"stop"`) {
		t.Fatalf("no stop line in %q", rest)
	}
	if _, err := os.Stat(filepath.Join(dir, relay.DBFile)); err != nil {
		t.Fatal(err)
	}
}
