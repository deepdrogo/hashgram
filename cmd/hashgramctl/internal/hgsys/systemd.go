// Package hgsys drives systemd and inspects the host.
//
// hashgramctl shells out to systemctl rather than talking to systemd over
// D-Bus. The commands are the ones an operator would type themselves, so when
// something goes wrong the failure is reproducible by hand, and hashgramctl
// stays usable on a machine where the D-Bus socket is not reachable from the
// service account.
package hgsys

import (
	"bufio"
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"time"
)

// Unit names for the Hashgram services.
const (
	UnitChain   = "hashgramd.service"
	UnitNode    = "hashgram-node.service"
	UnitIndexer = "hashgram-indexer.service"
	UnitSafety  = "hashgram-safety.service"
	UnitCall    = "coturn.service"
)

// Systemctl runs a systemctl subcommand.
func Systemctl(ctx context.Context, args ...string) (string, error) {
	cmd := exec.CommandContext(ctx, "systemctl", args...)
	out, err := cmd.CombinedOutput()
	return strings.TrimSpace(string(out)), err
}

// UnitState is a unit's runtime state.
type UnitState struct {
	Name      string
	Exists    bool
	Active    string // active, inactive, failed, activating
	Enabled   string // enabled, disabled, static, masked
	MainPID   int
	Since     string
	SubState  string
	MemoryMax string
}

// Running reports whether the unit is active.
func (u UnitState) Running() bool { return u.Active == "active" }

// Inspect reads a unit's state.
func Inspect(ctx context.Context, unit string) UnitState {
	state := UnitState{Name: unit}

	props, err := Systemctl(ctx, "show", unit,
		"--property=LoadState,ActiveState,SubState,UnitFileState,MainPID,ActiveEnterTimestamp,MemoryMax")
	if err != nil && props == "" {
		return state
	}

	for _, line := range strings.Split(props, "\n") {
		key, value, found := strings.Cut(strings.TrimSpace(line), "=")
		if !found {
			continue
		}
		switch key {
		case "LoadState":
			state.Exists = value == "loaded"
		case "ActiveState":
			state.Active = value
		case "SubState":
			state.SubState = value
		case "UnitFileState":
			state.Enabled = value
		case "MainPID":
			state.MainPID, _ = strconv.Atoi(value)
		case "ActiveEnterTimestamp":
			state.Since = value
		case "MemoryMax":
			state.MemoryMax = value
		}
	}
	return state
}

// Start starts a unit. Stop, Restart, Enable and Disable below are the same
// shape and drive the corresponding systemctl verb.
func Start(ctx context.Context, unit string) error   { return run(ctx, "start", unit) }
func Stop(ctx context.Context, unit string) error    { return run(ctx, "stop", unit) }
func Restart(ctx context.Context, unit string) error { return run(ctx, "restart", unit) }
func Enable(ctx context.Context, unit string) error  { return run(ctx, "enable", unit) }
func Disable(ctx context.Context, unit string) error { return run(ctx, "disable", unit) }

// DaemonReload reloads unit files.
func DaemonReload(ctx context.Context) error { return run(ctx, "daemon-reload") }

func run(ctx context.Context, args ...string) error {
	out, err := Systemctl(ctx, args...)
	if err != nil {
		return fmt.Errorf("systemctl %s: %v\n%s", strings.Join(args, " "), err, out)
	}
	return nil
}

// Journal streams a unit's logs.
func Journal(ctx context.Context, unit string, lines int, follow bool) error {
	args := []string{"-u", unit, "-n", strconv.Itoa(lines), "--no-pager"}
	if follow {
		args = append(args, "-f")
	}
	cmd := exec.CommandContext(ctx, "journalctl", args...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	return cmd.Run()
}

// DiskUsage reports free and total bytes for the filesystem holding a path.
func DiskUsage(path string) (free, total uint64, err error) {
	var st syscall.Statfs_t
	if err := syscall.Statfs(path, &st); err != nil {
		return 0, 0, err
	}
	// Only Bsize needs checking: on Linux, Statfs_t.Blocks and Bavail are
	// already uint64, so comparing them against zero is dead code that
	// staticcheck rightly rejects. Bsize is int64 and is the one value that
	// could be non-positive on a filesystem reporting nonsense, which would
	// otherwise turn into an enormous free-space figure in preflight output.
	if st.Bsize <= 0 {
		return 0, 0, fmt.Errorf(
			"statfs on %s reported a block size of %d", path, st.Bsize)
	}
	// #nosec G115 -- checked positive immediately above.
	bsize := uint64(st.Bsize)
	return st.Bavail * bsize, st.Blocks * bsize, nil
}

// DirSize sums the size of every regular file under a path.
func DirSize(path string) (uint64, error) {
	var total uint64
	err := filepath.Walk(path, func(_ string, info os.FileInfo, err error) error {
		if err != nil {
			// A file that disappeared mid-walk is normal on a running node;
			// skip it rather than failing the whole measurement.
			return nil
		}
		if info.Mode().IsRegular() && info.Size() > 0 {
			// #nosec G115 -- checked positive above.
			total += uint64(info.Size())
		}
		return nil
	})
	return total, err
}

// ClockSynchronised reports whether the system clock is synchronised.
//
// A validator with a badly wrong clock proposes blocks its peers reject, and
// the symptom looks like a networking fault rather than a clock fault, which
// is why this is a preflight check rather than something to discover later.
func ClockSynchronised(ctx context.Context) (bool, string) {
	out, err := exec.CommandContext(ctx, "timedatectl", "show",
		"--property=NTPSynchronized", "--value").Output()
	if err != nil {
		return false, "timedatectl is unavailable; clock synchronisation could not be verified"
	}
	value := strings.TrimSpace(string(out))
	if value == "yes" {
		return true, "system clock is NTP synchronised"
	}
	return false, "system clock is NOT NTP synchronised"
}

// FirewallActive reports whether ufw is active, and its rule summary.
func FirewallActive(ctx context.Context) (bool, string) {
	out, err := exec.CommandContext(ctx, "ufw", "status").Output()
	if err != nil {
		return false, "ufw is unavailable; firewall state could not be verified"
	}
	text := string(out)
	return strings.Contains(text, "Status: active"), strings.TrimSpace(text)
}

// ListeningSockets returns the listening TCP sockets as reported by ss.
//
// Used by the preflight check that looks for administrative ports bound to
// anything other than loopback.
func ListeningSockets(ctx context.Context) ([]string, error) {
	out, err := exec.CommandContext(ctx, "ss", "-tlnH").Output()
	if err != nil {
		return nil, fmt.Errorf("ss is unavailable; listening sockets could not be inspected: %w", err)
	}

	var sockets []string
	scanner := bufio.NewScanner(strings.NewReader(string(out)))
	for scanner.Scan() {
		fields := strings.Fields(scanner.Text())
		if len(fields) >= 4 {
			sockets = append(sockets, fields[3])
		}
	}
	return sockets, scanner.Err()
}

// FileMode returns a file's permission bits.
func FileMode(path string) (os.FileMode, error) {
	info, err := os.Stat(path)
	if err != nil {
		return 0, err
	}
	return info.Mode().Perm(), nil
}

// Uptime returns the host uptime.
func Uptime() (time.Duration, error) {
	raw, err := os.ReadFile("/proc/uptime")
	if err != nil {
		return 0, err
	}
	fields := strings.Fields(string(raw))
	if len(fields) == 0 {
		return 0, fmt.Errorf("unexpected /proc/uptime format")
	}
	secs, err := strconv.ParseFloat(fields[0], 64)
	if err != nil {
		return 0, err
	}
	return time.Duration(secs) * time.Second, nil
}

// HumanBytes renders a byte count for operator output.
//
// Generic over int64 and uint64 because both appear naturally: os.FileInfo
// reports an int64 size while DiskUsage reports uint64 totals. Accepting only
// one forced a conversion at half the call sites, which was both noise and
// the one place such a conversion could go wrong.
func HumanBytes[T int64 | uint64](n T) string {
	const unit = 1024
	if n < unit {
		return fmt.Sprintf("%d B", n)
	}
	suffixes := []string{"KiB", "MiB", "GiB", "TiB", "PiB"}
	value := float64(n)
	for _, s := range suffixes {
		value /= unit
		if value < unit {
			return fmt.Sprintf("%.1f %s", value, s)
		}
	}
	return fmt.Sprintf("%.1f EiB", value/unit)
}
