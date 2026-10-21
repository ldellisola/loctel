# Checking LogQL against the real Loki

`tests/logql.rs` was built by running thousands of queries and log lines through
Loki's own pipeline (v3.7.9, 2026-10-21) next to loctel's, and comparing
`kept` / `dropped` / `parse error`. Do the same when you change LogQL semantics:
reading `syntax.y` tells you the grammar, but only running it tells you what
yacc's conflict resolution, the `jsonparser` library and the logfmt decoder do.
Go is installed and `proxy.golang.org` is reachable. It costs about 2 GB of disk
(delete the scratch directory afterwards) and a few minutes the first time.

## Set up (in the scratchpad, not in the repository)

```bash
mkdir lokirun && cd lokirun
export GOFLAGS=-mod=mod GOPATH=$PWD/gopath GOCACHE=$PWD/gocache GOMODCACHE=$PWD/gopath/pkg/mod
printf 'module lokirun\ngo 1.24\n' > go.mod
go get github.com/grafana/loki/v3@latest
# Loki's go.mod has `replace` lines that only apply to Loki itself; copy them or
# the build fails inside dskit with "memberlist.NodeState undefined":
grep '^replace' gopath/pkg/mod/github.com/grafana/loki/v3@*/go.mod | grep -v '=> \./' >> go.mod
```

(Do not run `go mod tidy` with the module cache inside the module: it chokes. The
`GOPATH` inside the working directory is only for easy cleanup.)

## The Go side

Reads `query<TAB>line<TAB>k=v,k=v` rows (`<NL>` and `<TAB>` stand for those
characters) and prints `KEPT`, `DROPPED` or `PARSE ERROR` for each. Two things
are easy to miss: `Pipeline()` does not evaluate the selector, so check
`expr.Matchers()` yourself; and a numeric label filter that cannot read its label
keeps the line with `__error__="LabelFilterErr"`, which the harness reports as
`KEPT-FILTERERR` because loctel deliberately says false there.

```go
package main

import (
	"bufio"
	"fmt"
	"os"
	"strings"

	"github.com/grafana/loki/v3/pkg/logql/syntax"
	"github.com/prometheus/prometheus/model/labels"
)

func main() {
	f, _ := os.Open(os.Args[1])
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 1<<20), 1<<24)
	conv := strings.NewReplacer("<NL>", "\n", "<TAB>", "\t")
	for sc.Scan() {
		parts := strings.Split(sc.Text(), "\t")
		q, line := conv.Replace(parts[0]), conv.Replace(parts[1])
		ls := []labels.Label{{Name: "service_name", Value: "svc"}, {Name: "level", Value: "info"}}
		if len(parts) > 2 && parts[2] != "" {
			for _, kv := range strings.Split(parts[2], ",") {
				i := strings.Index(kv, "=")
				ls = append(ls, labels.Label{Name: kv[:i], Value: kv[i+1:]})
			}
		}
		fmt.Println(run(q, line, ls))
	}
}

func run(q, line string, ls []labels.Label) string {
	expr, err := syntax.ParseLogSelector(q, true)
	if err != nil {
		return "PARSE ERROR"
	}
	pipe, err := expr.Pipeline()
	if err != nil {
		return "PARSE ERROR"
	}
	lbls := labels.New(ls...)
	for _, m := range expr.Matchers() {
		if !m.Matches(lbls.Get(m.Name)) {
			return "DROPPED"
		}
	}
	_, res, ok := pipe.ForStream(lbls).Process(0, []byte(line), labels.EmptyLabels())
	if !ok {
		return "DROPPED"
	}
	if res.Labels().Get("__error__") == "LabelFilterErr" {
		return "KEPT-FILTERERR"
	}
	return "KEPT"
}
```

## The loctel side

A scratch integration test (do not commit it) that reads the same file and
builds a `LogRecord` from each row: the labels as log attributes, a resource with
`service.name = "svc"`, `Severity::Info` (so `level` is `info`, as on the Go
side), the line as the body. It prints the same three words. Leave empty label
values out of the rows: Loki keeps a label with an empty value, loctel treats it
as absent.

## What should differ, and nothing else

Generate rows from a small grammar (random label-filter expressions joined by
`and`, `or`, `,` and spaces, random bodies in JSON and logfmt, random stream
labels) and compare. As of this writing, 120,000 random rows and about 5,000
hand-written ones differ only here (all of them are in `docs/areas/query.md`,
"Where loctel differs from Loki"):

- features loctel says "not supported yet" to;
- numeric label filters on a value that is not a number (Loki keeps the line);
- a label filter written before `| json` or `| logfmt` (Loki's pushdown also
  applies it to the extracted label);
- an empty regex in a label filter (Loki's simplification matches everything);
- selectors Loki rejects as "empty-compatible" (`{app=~".*"}`), keywords as label
  names, `d`, `w` and `y` duration units, a `\xff` escape, `1e999`.
