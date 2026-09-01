package relay

import (
	"io"
	"os"
	"strconv"
	"strings"
	"sync"
	"time"
)

// Structured logs: one JSON object per line on stderr, the Rust relay's shape
// ({"ts":…,"level":…,k:v…}). They never carry a channel id, a device id or a client
// address (design §7.5): only verbs, statuses and timings.

var (
	logMu  sync.Mutex
	logOut io.Writer = os.Stderr
)

// SetLogOutput redirects the logs (tests); nil restores stderr.
func SetLogOutput(w io.Writer) {
	if w == nil {
		w = os.Stderr
	}
	logMu.Lock()
	logOut = w
	logMu.Unlock()
}

func escape(s string) string {
	var b strings.Builder
	for _, c := range s {
		switch {
		case c == '"':
			b.WriteString(`\"`)
		case c == '\\':
			b.WriteString(`\\`)
		case c < 0x20:
			b.WriteString(`\u00`)
			b.WriteByte("0123456789abcdef"[c>>4])
			b.WriteByte("0123456789abcdef"[c&15])
		default:
			b.WriteRune(c)
		}
	}
	return b.String()
}

// logEvent writes {"ts":…,"level":level,k:v…}; kv alternates keys and string values.
func logEvent(level string, kv ...string) {
	var b strings.Builder
	b.WriteString(`{"ts":`)
	b.WriteString(strconv.FormatInt(time.Now().UnixMilli(), 10))
	b.WriteString(`,"level":"` + escape(level) + `"`)
	for i := 0; i+1 < len(kv); i += 2 {
		b.WriteString(`,"` + escape(kv[i]) + `":"` + escape(kv[i+1]) + `"`)
	}
	b.WriteString("}\n")
	logMu.Lock()
	defer logMu.Unlock()
	// A full or closed stderr must never take the relay down.
	_, _ = io.WriteString(logOut, b.String())
}

// LogEvent is logEvent for the CLI.
func LogEvent(level string, kv ...string) { logEvent(level, kv...) }
