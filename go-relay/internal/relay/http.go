package relay

import (
	"context"
	"errors"
	"io"
	"log"
	"net"
	"net/http"
	"strconv"
	"sync"
	"time"
)

// HTTPLimits are the transport's limits (the handler's own are in Config).
type HTTPLimits struct {
	MaxBody        uint64
	HeaderTimeout  time.Duration // to send a request's headers, or start the next one
	BodyTimeout    time.Duration // to send a request's body
	ConnTimeout    time.Duration // the longest life of one connection
	MaxConnections int
}

// LimitsFor is the Rust relay's transport limits for cfg.
func LimitsFor(cfg Config) HTTPLimits {
	return HTTPLimits{
		MaxBody:        cfg.MaxBody(),
		HeaderTimeout:  10 * time.Second,
		BodyTimeout:    60 * time.Second,
		ConnTimeout:    300 * time.Second,
		MaxConnections: 512,
	}
}

// Server is the HTTP/1.1 server around a Relay: body limits, timeouts, a connection
// cap, CORS and the hourly prune sweep. TLS is a proxy's job (deploy/README.md).
type Server struct {
	mu    sync.Mutex // one request at a time through the handler
	relay *Relay
	lim   HTTPLimits
	now   func() uint64
}

// NowMS is the wall clock in Unix milliseconds.
func NowMS() uint64 { return uint64(time.Now().UnixMilli()) }

// NewServer wraps relay.
func NewServer(relay *Relay, lim HTTPLimits) *Server {
	return &Server{relay: relay, lim: lim, now: NowMS}
}

func (s *Server) reply(w http.ResponseWriter, status int, contentType string, body []byte) {
	h := w.Header()
	h.Set("Access-Control-Allow-Origin", "*")
	h.Set("Content-Type", contentType)
	h.Set("Content-Length", strconv.Itoa(len(body)))
	w.WriteHeader(status)
	_, _ = w.Write(body)
}

func logRequest(verb string, status int, started time.Time) {
	logEvent("info", "event", "request", "verb", verb, "status", strconv.Itoa(status),
		"ms", strconv.FormatInt(time.Since(started).Milliseconds(), 10))
}

// ServeHTTP answers one request.
func (s *Server) ServeHTTP(w http.ResponseWriter, req *http.Request) {
	started := time.Now()
	defer func() {
		if p := recover(); p != nil {
			if p == http.ErrAbortHandler {
				panic(p)
			}
			logEvent("error", "event", "panic")
			s.reply(w, 500, "application/cbor", errorBody("internal"))
		}
	}()
	if req.Method == http.MethodOptions {
		h := w.Header()
		h.Set("Access-Control-Allow-Methods", "POST")
		h.Set("Access-Control-Allow-Headers", "content-type")
		h.Set("Access-Control-Max-Age", "86400")
		s.reply(w, 204, "text/plain", nil)
		return
	}
	// The raw path, as sent: never percent-decoded or cleaned.
	path := req.URL.EscapedPath()
	if req.ContentLength > 0 && uint64(req.ContentLength) > s.lim.MaxBody {
		logRequest("-", 413, started)
		s.reply(w, 413, "application/cbor", errorBody("too_large"))
		return
	}
	_ = http.NewResponseController(w).SetReadDeadline(time.Now().Add(s.lim.BodyTimeout))
	body, err := io.ReadAll(http.MaxBytesReader(w, req.Body, int64(min(s.lim.MaxBody, 1<<62))))
	if err != nil {
		var tooBig *http.MaxBytesError
		var ne net.Error
		switch {
		case errors.As(err, &tooBig):
			logRequest("-", 413, started)
			s.reply(w, 413, "application/cbor", errorBody("too_large"))
		case errors.As(err, &ne) && ne.Timeout():
			logRequest("-", 408, started)
			s.reply(w, 408, "application/cbor", errorBody("timeout"))
		default:
			logRequest("-", 400, started)
			s.reply(w, 400, "application/cbor", errorBody("bad_request"))
		}
		return
	}
	s.mu.Lock()
	r := s.relay.Handle(req.Method, path, body, s.now())
	s.mu.Unlock()
	logRequest(r.Verb, r.Status, started)
	s.reply(w, r.Status, r.ContentType, r.Body)
}

// Sweep runs the relay's prune sweep once.
func (s *Server) Sweep() {
	s.mu.Lock()
	n, err := s.relay.Sweep(s.now())
	s.mu.Unlock()
	if err != nil {
		logEvent("error", "event", "sweep", "error", err.Error())
		return
	}
	logEvent("info", "event", "sweep", "pruned", strconv.FormatUint(n, 10))
}

// capListener closes connections past the cap at once and ends each connection after
// ConnTimeout, whatever it is doing.
type capListener struct {
	net.Listener
	sem  chan struct{}
	life time.Duration
}

type capConn struct {
	net.Conn
	once    sync.Once
	release func()
	timer   *time.Timer
}

func (c *capConn) Close() error {
	err := c.Conn.Close()
	c.once.Do(func() {
		c.timer.Stop()
		c.release()
	})
	return err
}

func (l *capListener) Accept() (net.Conn, error) {
	for {
		c, err := l.Listener.Accept()
		if err != nil {
			return nil, err
		}
		select {
		case l.sem <- struct{}{}:
		default:
			c.Close()
			continue
		}
		cc := &capConn{Conn: c, release: func() { <-l.sem }}
		cc.timer = time.AfterFunc(l.life, func() { cc.Close() })
		return cc, nil
	}
}

// discard swallows net/http's own error log: its lines are plain text and can name a
// client address, and logs here are JSON without addresses.
type discard struct{}

func (discard) Write(p []byte) (int, error) {
	logEvent("warn", "event", "http_error")
	return len(p), nil
}

// Serve serves on ln until ctx is done, then stops accepting and lets requests in
// flight finish (up to 10 s).
func (s *Server) Serve(ctx context.Context, ln net.Listener) error {
	srv := &http.Server{
		Handler:           s,
		ReadHeaderTimeout: s.lim.HeaderTimeout,
		IdleTimeout:       s.lim.HeaderTimeout,
		MaxHeaderBytes:    64 << 10,
		ErrorLog:          log.New(discard{}, "", 0),
	}
	srv.SetKeepAlivesEnabled(true)
	sweepCtx, stopSweep := context.WithCancel(ctx)
	defer stopSweep()
	go func() {
		s.Sweep()
		t := time.NewTicker(time.Hour)
		defer t.Stop()
		for {
			select {
			case <-sweepCtx.Done():
				return
			case <-t.C:
				s.Sweep()
			}
		}
	}()
	errc := make(chan error, 1)
	go func() {
		errc <- srv.Serve(&capListener{Listener: ln, sem: make(chan struct{}, s.lim.MaxConnections), life: s.lim.ConnTimeout})
	}()
	select {
	case err := <-errc:
		return err
	case <-ctx.Done():
	}
	shut, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	err := srv.Shutdown(shut)
	if errors.Is(err, context.DeadlineExceeded) {
		err = srv.Close()
	}
	return err
}

// Close waits for the request or sweep in progress and closes the relay's store.
func (s *Server) Close() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.relay.Close()
}
