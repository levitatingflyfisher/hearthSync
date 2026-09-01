package relay

// DayMS is one day in milliseconds.
const DayMS uint64 = 24 * 60 * 60 * 1000

// Config holds every limit the handler enforces (docs/reference/relay-protocol.md,
// "Limits" and "Rate limits"). The names are the vectors' config keys.
type Config struct {
	WindowMS          uint64
	MaxEnvelope       uint64
	MaxBatch          uint64
	MaxSnapshot       uint64
	ChannelQuota      uint64
	MaxTotalBytes     uint64
	MaxDevices        uint64
	MaxChannels       uint64
	MaxPullEntries    uint64
	MaxPullBytes      uint64
	RetainMS          uint64
	DeviceBurst       uint64
	DeviceIntervalMS  uint64
	EnrollBurst       uint64
	EnrollIntervalMS  uint64
	CreateBurst       uint64
	CreateIntervalMS  uint64
	ChannelBurst      uint64
	ChannelIntervalMS uint64
	IdleMS            uint64 // a channel with no write for this long is expired by the sweep
	MaxReaderNonces   uint64 // live read nonces one reader may hold in one channel
}

// DefaultConfig is the protocol's defaults.
func DefaultConfig() Config {
	return Config{
		WindowMS:          300_000,
		MaxEnvelope:       64*1024 + 1024, // an op's 64 KiB plus the envelope's framing
		MaxBatch:          64,
		MaxSnapshot:       8 << 20,
		ChannelQuota:      64 << 20,
		MaxTotalBytes:     8 << 30,
		MaxDevices:        32,
		MaxChannels:       1_000,
		MaxPullEntries:    512,
		MaxPullBytes:      4 << 20,
		RetainMS:          (90 + 30) * DayMS, // the kernel's horizon plus 30 days' grace
		DeviceBurst:       120,
		DeviceIntervalMS:  500,
		EnrollBurst:       16,
		EnrollIntervalMS:  60_000,
		CreateBurst:       32,
		CreateIntervalMS:  60_000,
		ChannelBurst:      600,
		ChannelIntervalMS: 100,
		IdleMS:            400 * DayMS,
		MaxReaderNonces:   16,
	}
}

// Set sets one limit by its protocol name; false for an unknown name.
func (c *Config) Set(name string, v uint64) bool {
	f := map[string]*uint64{
		"window_ms":           &c.WindowMS,
		"max_envelope":        &c.MaxEnvelope,
		"max_batch":           &c.MaxBatch,
		"max_snapshot":        &c.MaxSnapshot,
		"channel_quota":       &c.ChannelQuota,
		"max_total_bytes":     &c.MaxTotalBytes,
		"max_devices":         &c.MaxDevices,
		"max_channels":        &c.MaxChannels,
		"max_pull_entries":    &c.MaxPullEntries,
		"max_pull_bytes":      &c.MaxPullBytes,
		"retain_ms":           &c.RetainMS,
		"device_burst":        &c.DeviceBurst,
		"device_interval_ms":  &c.DeviceIntervalMS,
		"enroll_burst":        &c.EnrollBurst,
		"enroll_interval_ms":  &c.EnrollIntervalMS,
		"create_burst":        &c.CreateBurst,
		"create_interval_ms":  &c.CreateIntervalMS,
		"channel_burst":       &c.ChannelBurst,
		"channel_interval_ms": &c.ChannelIntervalMS,
		"idle_ms":             &c.IdleMS,
		"max_reader_nonces":   &c.MaxReaderNonces,
	}[name]
	if f == nil {
		return false
	}
	*f = v
	return true
}

// MaxBody is the HTTP body limit: the largest request (a snapshot, or a full batch of
// envelopes, whichever is larger) plus 64 KiB for its framing.
func (c *Config) MaxBody() uint64 {
	batch := satMul(c.MaxBatch, satAdd(c.MaxEnvelope, 8))
	return satAdd(max(c.MaxSnapshot, batch), 64*1024)
}

func satAdd(a, b uint64) uint64 {
	if a+b < a {
		return ^uint64(0)
	}
	return a + b
}

func satMul(a, b uint64) uint64 {
	if a != 0 && b > ^uint64(0)/a {
		return ^uint64(0)
	}
	return a * b
}
