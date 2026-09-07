package cmd

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"github.com/hashgram/hashgram/cmd/hashgramctl/internal/hgconfig"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"text/tabwriter"
	"time"
)

// commandContext returns a context with a sensible timeout for an
// interactive operator command.
func commandContext() (context.Context, context.CancelFunc) {
	return context.WithTimeout(context.Background(), 30*time.Second)
}

// out writes a table or JSON depending on --json.
type out struct {
	tw   *tabwriter.Writer
	json map[string]any
}

func newOut() *out {
	return &out{
		tw:   tabwriter.NewWriter(os.Stdout, 0, 4, 2, ' ', 0),
		json: map[string]any{},
	}
}

// row adds a labelled value.
func (o *out) row(label string, value any) {
	if flagJSON {
		o.json[jsonKey(label)] = value
		return
	}
	fmt.Fprintf(o.tw, "%s\t%v\n", label, value)
}

// section prints a heading. Ignored in JSON mode.
func (o *out) section(name string) {
	if flagJSON {
		return
	}
	fmt.Fprintf(o.tw, "\n%s\n", name)
}

// blank prints a blank line. Ignored in JSON mode.
func (o *out) blank() {
	if flagJSON {
		return
	}
	fmt.Fprintln(o.tw)
}

// raw prints a line verbatim. Ignored in JSON mode.
func (o *out) raw(s string) {
	if flagJSON {
		return
	}
	fmt.Fprintln(o.tw, s)
}

// set adds a JSON-only field.
func (o *out) set(key string, value any) {
	o.json[key] = value
}

// flush writes the accumulated output.
func (o *out) flush() error {
	if flagJSON {
		enc := json.NewEncoder(os.Stdout)
		enc.SetIndent("", "  ")
		return enc.Encode(o.json)
	}
	return o.tw.Flush()
}

func jsonKey(label string) string {
	k := strings.ToLower(strings.TrimSpace(label))
	k = strings.TrimSuffix(k, ":")
	k = strings.ReplaceAll(k, " ", "_")
	k = strings.ReplaceAll(k, "-", "_")
	k = strings.ReplaceAll(k, "/", "_")
	return k
}

// confirm asks the operator to type a specific word.
//
// Typing a specific word rather than "y" is deliberate for irreversible
// actions: it makes reflexive confirmation harder, which matters when the
// action is creating a network's immutable genesis.
func confirm(prompt, expected string) error {
	if flagYes {
		return nil
	}

	fmt.Printf("%s\nType %q to continue: ", prompt, expected)
	reader := bufio.NewReader(os.Stdin)
	answer, err := reader.ReadString('\n')
	if err != nil {
		return fmt.Errorf("reading confirmation: %w", err)
	}
	if strings.TrimSpace(answer) != expected {
		return fmt.Errorf("aborted: confirmation did not match")
	}
	return nil
}

// runHashgramd runs a hashgramd subcommand and returns its combined output.
func runHashgramd(ctx context.Context, args ...string) (string, error) {
	bin, err := hashgramdPath()
	if err != nil {
		return "", err
	}
	full := append([]string{"--home", paths.NodeHome}, args...)
	cmd := exec.CommandContext(ctx, bin, full...)
	raw, err := cmd.CombinedOutput()
	text := strings.TrimSpace(string(raw))
	if err != nil {
		return text, fmt.Errorf("hashgramd %s: %v\n%s", strings.Join(args, " "), err, text)
	}
	return text, nil
}

// queryJSON runs a hashgramd query and decodes its JSON output.
func queryJSON(ctx context.Context, out any, args ...string) error {
	full := append(args, "--node", flagRPC, "--output", "json")
	text, err := runHashgramd(ctx, full...)
	if err != nil {
		return err
	}
	return json.Unmarshal([]byte(text), out)
}

// fileExists reports whether a path exists.
func fileExists(path string) bool {
	_, err := os.Stat(path)
	return err == nil
}

// resolvePaths turns the three directory flags into absolute, cleaned paths.
//
// filepath.Abs applies filepath.Clean, which resolves "." and ".." segments
// lexically. It does not resolve symlinks, and deliberately so: a node home
// that is a symlink to a larger volume is a normal deployment, and following
// the link here would make every printed path disagree with the path the
// operator configured.
func resolvePaths(configDir, dataDir, nodeHome string) (hgconfig.Paths, error) {
	abs := func(label, p string) (string, error) {
		if strings.TrimSpace(p) == "" {
			return "", fmt.Errorf("%s must not be empty", label)
		}
		out, err := filepath.Abs(p)
		if err != nil {
			return "", fmt.Errorf("resolving %s %q: %w", label, p, err)
		}
		return out, nil
	}

	var (
		out hgconfig.Paths
		err error
	)
	if out.ConfigDir, err = abs("--config-dir", configDir); err != nil {
		return hgconfig.Paths{}, err
	}
	if out.DataDir, err = abs("--data-dir", dataDir); err != nil {
		return hgconfig.Paths{}, err
	}
	if out.NodeHome, err = abs("--home", nodeHome); err != nil {
		return hgconfig.Paths{}, err
	}
	return out, nil
}
