// Command hearth-relay-go serves the hearthSync relay protocol over HTTP/1.1: the Go
// implementation, held to the same vectors as the Rust hearth-relay.
//
// House CLI rules: results to stdout (only --help and --version print any), errors and
// logs to stderr; exit 0 success, 1 failure, 2 usage.
package main

import (
	"context"
	"errors"
	"fmt"
	"net"
	"net/netip"
	"os"
	"os/signal"
	"strconv"
	"syscall"

	"hearthsync/go-relay/internal/relay"
)

const version = "0.1.0"

const help = `hearth-relay-go: the hearthSync relay in Go (data-blind store-and-forward of sealed ops)

Usage: hearth-relay-go --data <DIR> [options]

Options:
  --data <DIR>               Directory for the database (must exist and be writable)
  --listen <ADDR>            Address to listen on, IP:port [default: 127.0.0.1:8080]
  --channel-quota <BYTES>    Storage per household channel [default: 67108864]
  --max-total-bytes <BYTES>  Storage for the whole relay [default: 8589934592]
  --max-channels <N>         Household channels the relay accepts [default: 1000]
  --retain-days <N>          Keep covered log entries this long [default: 120]
  --idle-days <N>            Expire a channel after this long with no write [default: 400]
  --max-devices <N>          Devices per household channel [default: 32]
  --max-reader-nonces <N>    Live read nonces per device and channel [default: 16]
  --channel-burst <N>        Requests a channel may burst to [default: 600]
  --channel-interval-ms <N>  One more channel request allowed every N ms [default: 100]
  -h, --help                 Print this help
  --version                  Print the version

It prints nothing while serving except structured JSON logs on stderr, which never
carry channel or device ids. Put a TLS proxy in front (see go-relay/deploy/README.md).
Stops on SIGTERM or SIGINT.
`

type args struct {
	data   string
	listen netip.AddrPort
	cfg    relay.Config
}

var errHelp, errVersion = errors.New("help"), errors.New("version")

func parse(argv []string) (*args, error) {
	a := &args{listen: netip.MustParseAddrPort("127.0.0.1:8080"), cfg: relay.DefaultConfig()}
	hasData := false
	for i := 0; i < len(argv); i++ {
		arg := argv[i]
		val := func() (string, error) {
			if i+1 >= len(argv) {
				return "", fmt.Errorf("%s needs a value", arg)
			}
			i++
			return argv[i], nil
		}
		num := func() (uint64, error) {
			v, err := val()
			if err != nil {
				return 0, err
			}
			n, err := strconv.ParseUint(v, 10, 64)
			if err != nil {
				return 0, fmt.Errorf("%s: not a number: %s", arg, v)
			}
			return n, nil
		}
		var err error
		switch arg {
		case "-h", "--help":
			return nil, errHelp
		case "--version":
			return nil, errVersion
		case "--data":
			a.data, err = val()
			hasData = true
		case "--listen":
			var v string
			if v, err = val(); err == nil {
				if a.listen, err = netip.ParseAddrPort(v); err != nil {
					err = fmt.Errorf("--listen: not an address: %s", v)
				}
			}
		case "--channel-quota":
			a.cfg.ChannelQuota, err = num()
		case "--max-total-bytes":
			a.cfg.MaxTotalBytes, err = num()
		case "--max-channels":
			a.cfg.MaxChannels, err = num()
		case "--max-devices":
			a.cfg.MaxDevices, err = num()
		case "--max-reader-nonces":
			a.cfg.MaxReaderNonces, err = num()
		case "--channel-burst":
			a.cfg.ChannelBurst, err = num()
		case "--channel-interval-ms":
			a.cfg.ChannelIntervalMS, err = num()
		case "--retain-days":
			var d uint64
			if d, err = num(); err == nil {
				if d > ^uint64(0)/relay.DayMS {
					err = errors.New("--retain-days: too large")
				}
				a.cfg.RetainMS = d * relay.DayMS
			}
		case "--idle-days":
			var d uint64
			if d, err = num(); err == nil {
				if d > ^uint64(0)/relay.DayMS {
					err = errors.New("--idle-days: too large")
				}
				a.cfg.IdleMS = d * relay.DayMS
			}
		default:
			err = fmt.Errorf("unknown argument: %s", arg)
		}
		if err != nil {
			return nil, err
		}
	}
	if !hasData {
		return nil, errors.New("--data <DIR> is required")
	}
	return a, nil
}

func run(argv []string) int {
	a, err := parse(argv)
	switch {
	case errors.Is(err, errHelp):
		fmt.Print(help)
		return 0
	case errors.Is(err, errVersion):
		fmt.Println("hearth-relay-go " + version)
		return 0
	case err != nil:
		fmt.Fprintf(os.Stderr, "hearth-relay-go: %s\nTry 'hearth-relay-go --help'.\n", err)
		return 2
	}
	if st, err := os.Stat(a.data); err != nil || !st.IsDir() {
		fmt.Fprintf(os.Stderr, "hearth-relay-go: --data %s: not a directory\n", a.data)
		return 1
	}
	r, err := relay.Open(a.data, a.cfg, false)
	if err != nil {
		fmt.Fprintf(os.Stderr, "hearth-relay-go: cannot open the database in %s: %s\n", a.data, err)
		return 1
	}
	ln, err := net.Listen("tcp", a.listen.String())
	if err != nil {
		r.Close()
		fmt.Fprintf(os.Stderr, "hearth-relay-go: cannot listen on %s: %s\n", a.listen, err)
		return 1
	}
	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGTERM, syscall.SIGINT)
	defer stop()
	relay.LogEvent("info", "event", "start", "listen", ln.Addr().String(), "version", version)
	srv := relay.NewServer(r, relay.LimitsFor(a.cfg))
	err = srv.Serve(ctx, ln)
	if cerr := srv.Close(); err == nil {
		err = cerr
	}
	if err != nil {
		relay.LogEvent("error", "event", "stop", "error", err.Error())
		return 1
	}
	relay.LogEvent("info", "event", "stop")
	return 0
}

func main() { os.Exit(run(os.Args[1:])) }
