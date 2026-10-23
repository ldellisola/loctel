---
name: run-and-screenshot
description: "Run loctel on Linux and look at it: headless with meshgen data, or the real main window under Xvfb with a PNG you can open. Use whenever you need to see the app work, check that a UI change shows up in the real window (not only in the screenshot tests), or send fake traces and logs to a running loctel."
---

# Run loctel and take a screenshot

You are on Linux with no display and no GPU. Two ways to run loctel; pick the
lightest that answers your question. Run `bash script/setup-linux.sh` first if
a tool below is missing (it is idempotent).

## 1. Headless: no window, ports and data only

Right for ingest, store, MCP and engine work. Port 0 picks a free port, and
the listener lines on stdout say which (the line after them, `"event":"rules"`,
says where the discard rules file is and how many rules it holds). Point
`--settings` and `--rules` at scratch files so you never touch the real ones.

```bash
cargo build -p loctel-app -p loctel-meshgen
target/debug/loctel --headless --grpc-port 0 --http-port 0 --mcp-port 0 \
  --settings /tmp/loctel-settings.toml --rules /tmp/loctel-rules.toml > /tmp/loctel-lines.txt &
sleep 1; cat /tmp/loctel-lines.txt        # {"event":"listener","name":"otlp-grpc",...,"address":"127.0.0.1:41727"}
target/debug/meshgen --scenario checkout-failure --transport grpc --endpoint http://127.0.0.1:41727
kill %1                                    # SIGTERM or SIGINT: clean exit 0
```

Unless you pass `--no-spill`, loctel keeps older traces and logs in temporary segment
files in `$TMPDIR/loctel/run-<pid>-<secs>/`, deletes that folder when it exits, and
deletes the folders of runs that were killed the next time it starts (the cap is
`disk_cap_bytes` of the settings file, 4 GiB unless the file says otherwise). The store
only moves data out of memory when something calls it (an ingest, a read, or the
window's refresh every 500 ms), so a headless run that nobody talks to has no files yet:
send it a trace, wait for it to go cold (10 s of quiet and 30 s more; logs go when a chunk of
65,536 or ten seconds of them seals), send anything again (that ingest runs the step) and look:

```bash
export TMPDIR=/tmp/loctel-scratch; mkdir -p $TMPDIR      # so the folder is easy to find, and yours
target/debug/loctel --headless ... &                      # as above (with the same TMPDIR)
target/debug/meshgen --scenario huge-trace --transport http-protobuf --endpoint http://127.0.0.1:41727
sleep 45                                                  # 10 s quiet to be complete, 30 s more to spill
target/debug/meshgen --scenario checkout-success --transport grpc --endpoint http://127.0.0.1:41727  # any ingest runs the spill step
sleep 2; ls -la $TMPDIR/loctel/run-*/                     # traces-000000.seg, about 1.7 MB for 100,000 spans
kill %1; ls $TMPDIR/loctel/                               # the run folder is gone
```

`get_status` over MCP reports `disk_bytes`, `disk_cap_bytes` and `spill_error`. Use
`--no-spill` when you want the old all-in-memory behaviour (to compare, or to rule the
files out when a bug might be theirs); a store with no files answers every call the same
way, only without the lower half of "Storage, end to end" in `docs/areas/store.md`.

To see a discard rule work, add one to the rules file (a new file holds no
rules, only the example commented out; `docs/areas/rules.md`), wait for the
engine to notice the edit (it looks every 2 seconds), send the `health-checks`
scenario and look at what is stored. `get_status` over MCP shows the rule counts:

```bash
cat >> /tmp/loctel-rules.toml <<'RULE'
[[rule]]
name = "Health checks"
enabled = true
language = "traceql"
query = '{ span.url.path = "/health" }'
RULE
sleep 3
target/debug/meshgen --scenario health-checks --transport grpc --endpoint http://127.0.0.1:41727
```

Scenarios, transports and flags are in `docs/areas/testing.md`. The MCP address
is the `mcp` line; `docs/areas/mcp.md` has the tools.

## 2. The real window, in one command

```bash
cargo xtask run-shot
```

It builds `loctel` and `meshgen`, runs `loctel` under `xvfb-run` (a 1440 by 900
virtual screen, the window's default size, so the window fills it) with every
port 0 and a scratch settings file, waits for the window and two seconds for
its first frame, sends it the failing checkout trace (`meshgen --scenario
checkout-failure`, to the OTLP HTTP address from the listener lines), waits two
more seconds for the window to read it, captures the screen and writes
`target/run-shot.png`. **Open the PNG with the Read tool.** Expect the main
window (light theme: Xvfb reports the light appearance): the sidebar with the
status box at the bottom, `Map & traces` in the toolbar, the service map with
the failing services outlined in red above the trace table with one failed
`POST /api/checkout` (1.84 s, 15 spans). If the window is missing or black,
the script nudges it once and then fails with a message; do not paper over that.

## 3. The real window with other data (or a taken port)

`run-shot` sends one scenario, `checkout-failure`, once. For another scenario,
a steady stream or a taken port, fix the ports so meshgen can find them, and
take the shot after the data arrived. The window reads the store every 500 ms
(the status box once a second, and the rates average the last five seconds), so
wait 2 seconds after sending.

```bash
cargo build -p loctel-app -p loctel-meshgen
cat > /tmp/shot.sh <<'SCRIPT'
# Runs inside Xvfb, from the repo root. $1 = output PNG
export XDG_RUNTIME_DIR=$(mktemp -d)
settings=$(mktemp -d)/settings.toml
target/debug/loctel --grpc-port 14317 --http-port 14318 --mcp-port 14319 \
  --settings "$settings" > /tmp/loctel-lines.txt 2>/dev/null &
app=$!
sleep 3                                  # window up, first frame drawn
target/debug/meshgen --scenario checkout-failure --transport grpc \
  --endpoint http://127.0.0.1:14317
sleep 2                                  # one refresh with the new numbers
import -window root "$1"
kill $app
SCRIPT
xvfb-run -a -s "-screen 0 1440x900x24" bash /tmp/shot.sh /tmp/mesh-shot.png
```

Open `/tmp/mesh-shot.png` with the Read tool (the screen is the window's default
size, 1440 by 900, so a smaller one would cut the window). After the `checkout-failure` scenario expect 3 spans/s in the
status box and one failed trace in the table. To see a port problem, hold
a port before loctel starts (`python3 -m http.server 14318 --bind 127.0.0.1 &`,
then `sleep 1`): the status box reads `Port problem: otlp-http (address in use)` and
that row of the status box says `Failed` in red with the OS error.

### The trace view in the real window

The main window has four screens: the home screen above, the trace view that a
click on a row of the trace table opens (Esc goes back), and Logs and Discard
rules, which the next section reaches. To capture the trace view, click the first
row before the capture. In a 1440 by 900 window the first
row of the table is at x 432, y 640 (the window's logical pixels are the
screen's), so add these two lines to `shot.sh` before the `import`:

```bash
xdotool mousemove 432 640 click 1
sleep 2                                  # the trace screen reads the store, then draws
```

External nodes such as `card-gateway` (and the red edge to it) appear on the
map only once their trace is complete, 10 seconds after its last span
(`docs/areas/store.md`, "Service map"). A shot taken 2 seconds after sending
has no `card-gateway` node; the screenshot tests, whose `ManualClock` is past
the quiet period, do. That difference is expected.

Expect the trace's header (the red `Failed in payments-api · POST card-gateway
+1` badge), the 15-row waterfall with the failed-in `POST` selected and the
inspector on the right, and `Receiving spans…` at the top right until the trace
has been quiet for 10 seconds (then `Complete`). Only `xdotool` clicks reach
the real window; the screenshot tests drive their window with `Driver` instead
(`docs/areas/ui.md`).

### Logs and Discard rules in the real window

The sidebar lists three screens: `Map & traces` (⌘1), `Logs` (⌘2) and `Discard
rules` (⌘3), and ⌘, opens the Settings window. In the Linux window GPUI's `cmd` is
the Super key, so `xdotool key super+2` is ⌘2 (the screenshot tests send the same
keystroke as `cmd-2`). There is no window manager under `xvfb-run` and the window
fills the 1440 by 900 screen, so the pointer is always over it; the script still
moves it there first, so the keys reach the window whatever the X server's focus is.
A click on the sidebar works too: in the same window the three entries are at x 120
and y 79, 115 and 151.

The script gives the Discard rules screen a rule that has something to drop (a
rules file written before loctel starts), and sends the failing checkout, the health
checks and 300 log lines, so Logs has lines with and without a trace and the rule's
card has a count:

```bash
cargo build -p loctel-app -p loctel-meshgen
mkdir -p /tmp/screens
cat > /tmp/screens.sh <<'SCRIPT'
# Runs inside Xvfb, from the repo root. $1 = an existing output directory
export XDG_RUNTIME_DIR=$(mktemp -d)
work=$(mktemp -d)
cat > "$work/rules.toml" <<'RULE'
[[rule]]
name = "Health checks"
enabled = true
language = "traceql"
query = '{ span.url.path = "/health" }'
RULE
target/debug/loctel --grpc-port 14317 --http-port 14318 --mcp-port 14319 \
  --settings "$work/settings.toml" --rules "$work/rules.toml" \
  > /tmp/loctel-lines.txt 2>/dev/null &
app=$!
sleep 3                                  # window up, first frame drawn
for scenario in checkout-failure health-checks; do
  target/debug/meshgen --scenario "$scenario" --transport http-json \
    --endpoint http://127.0.0.1:14318
done
target/debug/meshgen --scenario log-flood --size 300 --transport http-json \
  --endpoint http://127.0.0.1:14318
sleep 3                                  # the screens read the store every 500 ms
xdotool mousemove 900 450                # the pointer over the window, the keys follow
xdotool key super+2                      # ⌘2: Logs
sleep 2
import -window root "$1/logs.png"
xdotool key super+3                      # ⌘3: Discard rules
sleep 2
import -window root "$1/rules.png"
kill $app
SCRIPT
xvfb-run -a -s "-screen 0 1440x900x24" bash /tmp/screens.sh /tmp/screens
```

Open both PNGs with the Read tool. Expect, in `logs.png`, `Logs` lit in the sidebar
and in the toolbar with the `Live tail` pill, the LogQL bar with its grey example,
the four level toggles, `307 lines · newest first` (the checkout's 7 lines and the
300 of the flood; the health checks' 100 lines went with their dropped traces),
the histogram with a single bar at the right end (everything arrived in the last
few seconds, the checkout's errors red at its foot) and the failing checkout's
seven lines (three `ERR`, one `WRN`, a trace link in the last column) on top of the
flood's `INF` and `DBG` lines. Expect, in `rules.png`, `Discard rules` lit,
`38% of incoming data dropped`, and one card: the switch on, `Health checks` with
its `TRACEQL` chip and `100 spans dropped in 100 traces`. For a screen that needs
clicks (an expanded line, the rules editor), add `xdotool mousemove X Y click 1`
lines as in the trace view above and take X and Y from a first capture (the
window's logical pixels are the screen's). For exact positions and states, with no
guessing, use a screenshot test and `Driver` (`docs/areas/ui.md`).

## Things that bite

- **No display** (`DISPLAY` and `WAYLAND_DISPLAY` unset): `loctel` without
  `--headless` says so on stderr and runs headless. Use `xvfb-run`.
- `libEGL warning: DRI3 ...` on stderr under Xvfb is noise: software rendering
  works. If rendering fails with "No usable headless GPU adapter found", Vulkan
  or EGL is missing: run `script/setup-linux.sh`.
- **Closing the window quits loctel**, and so do SIGINT and SIGTERM: either way
  the engine is shut down and the exit status is 0 (as with `--headless`).
- The first frame takes about two seconds, so never capture sooner than that
  after the window appears.
- This is the Linux development window, not the product. On a Mac the app is a
  menu bar item whose popover (a click on the icon) has `Open loctel`, which
  opens the same window (and shows loctel in the Dock while it is open): all of
  that is unverified until a milestone check on a real Mac.
- To compare pixels in a test instead of looking, write a screenshot test: see
  `docs/areas/ui.md` and the `update-screenshots` skill.
