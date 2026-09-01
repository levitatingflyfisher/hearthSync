package relay

import (
	"encoding/binary"
	"runtime"
	"testing"
)

// TestWorstCaseMemoryOfTheInMemoryTables fills the nonce and bucket tables to the bound
// the defaults allow (max_channels x max_devices readers, each holding
// max_reader_nonces live nonces, each with a device bucket, plus a bucket per channel)
// and measures the heap they take. It is the number ADR 0013 quotes against the systemd
// unit's MemoryMax=512M.
func TestWorstCaseMemoryOfTheInMemoryTables(t *testing.T) {
	if testing.Short() {
		t.Skip("fills the tables to their bound")
	}
	cfg := DefaultConfig()
	r := &Relay{cfg: cfg, buckets: map[bucketKey]*bucket{}, nonces: map[readerKey][]nonceExp{}}
	var before, after runtime.MemStats
	runtime.GC()
	runtime.ReadMemStats(&before)
	const now = 1_727_000_000_000
	for c := uint64(0); c < cfg.MaxChannels; c++ {
		var ch ID
		binary.BigEndian.PutUint64(ch[:], c)
		for d := uint64(0); d < cfg.MaxDevices; d++ {
			var dev ID
			binary.BigEndian.PutUint64(dev[:], d)
			for n := uint64(0); n < cfg.MaxReaderNonces; n++ {
				var nonce [16]byte
				binary.BigEndian.PutUint64(nonce[:], n)
				if err := r.useNonce(&ch, &dev, nonce, now, now); err != nil {
					t.Fatal(err)
				}
			}
			if err := r.deviceRate(&ch, &dev, now); err != nil {
				t.Fatal(err)
			}
		}
	}
	runtime.GC()
	runtime.ReadMemStats(&after)
	used := after.HeapAlloc - before.HeapAlloc
	readers := cfg.MaxChannels * cfg.MaxDevices
	t.Logf("%d readers, %d nonces, %d buckets: %.1f MiB live heap (%.0f B per reader)",
		readers, readers*cfg.MaxReaderNonces, len(r.buckets), float64(used)/(1<<20), float64(used)/float64(readers))
	if used > 128<<20 {
		t.Fatalf("the in-memory tables take %d bytes at the default bound; the unit allows 512M in all", used)
	}
	runtime.KeepAlive(r)
}
