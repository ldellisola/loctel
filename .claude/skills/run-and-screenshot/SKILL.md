---
name: run-and-screenshot
description: "Run loctel on Linux and look at it: headless with meshgen data, or the real status window under Xvfb with a PNG you can open. Use whenever you need to see the app work, check that a UI change shows up in the real window (not only in the screenshot tests), or send fake traces and logs to a running loctel."
---

# Run loctel and take a screenshot

You are on Linux with no display and no GPU. Two ways to run loctel; pick the
lightest that answers your question. Run `bash script/setup-linux.sh` first if
a tool below is missing (it is idempotent).

## 1. Headless: no window, ports and data only

Right for ingest, store, MCP and engine work. Port 0 picks a free port, and
the listener lines on stdout say which. Point `--settings` at a scratch file so
you never touch the real one.

```bash
cargo build -p loctel-app -p loctel-meshgen
target/debug/loctel --headless --grpc-port 0 --http-port 0 --mcp-port 0 \
  --settings /tmp/loctel-settings.toml > /tmp/loctel-lines.txt &
sleep 1; cat /tmp/loctel-lines.txt        # {"event":"listener","name":"otlp-grpc",...,"address":"127.0.0.1:41727"}
target/debug/meshgen --scenario checkout-failure --transport grpc --endpoint http://127.0.0.1:41727
kill %1                                    # SIGTERM or SIGINT: clean exit 0
```

Scenarios, transports and flags are in `docs/areas/testing.md`. The MCP address
is the `mcp` line; `docs/areas/mcp.md` has the tools.

## 2. The real window, in one command

```bash
cargo xtask run-shot
```

It builds `loctel`, runs it under `xvfb-run` (a 1280 by 800 virtual screen) with
every port 0 and a scratch settings file, waits for the window, captures the
screen and writes `target/run-shot.png`. **Open the PNG with the Read tool.**
The status window sits in the black screen at about (192, 100), 900 by 600:
a green dot and `Receiving · 0 spans/s · 0 logs/s`, four zeroed cards and the
three endpoints, each `Listening`. If the window is missing or black, the
script nudges it once and then fails with a message; do not paper over that.

## 3. The real window with meshgen data (or a taken port)

`run-shot` shows an empty loctel. To see data, ports must be fixed so meshgen
can find them, and the shot must be taken after the data arrived. The window
refreshes once a second and the rates average the last five seconds, so wait
2 seconds after sending.

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
xvfb-run -a -s "-screen 0 1280x800x24" bash /tmp/shot.sh /tmp/mesh-shot.png
```

Open `/tmp/mesh-shot.png` with the Read tool. After the `checkout-failure`
scenario expect 3 spans/s, 1 trace and 7 logs held. To see a port problem, hold
a port before loctel starts (`python3 -m http.server 14318 --bind 127.0.0.1 &`,
then `sleep 1`): the pill reads `Port problem: otlp-http (address in use)` and
that row says `Failed` in red with the OS error.

## Things that bite

- **No display** (`DISPLAY` and `WAYLAND_DISPLAY` unset): `loctel` without
  `--headless` says so on stderr and runs headless. Use `xvfb-run`.
- `libEGL warning: DRI3 ...` on stderr under Xvfb is noise: software rendering
  works. If rendering fails with "No usable headless GPU adapter found", Vulkan
  or EGL is missing: run `script/setup-linux.sh`.
- **Closing the window quits loctel.** `kill` (SIGTERM) also ends it, abruptly;
  only `--headless` shuts the engine down on a signal.
- The first frame takes about two seconds, so never capture sooner than that
  after the window appears.
- This is the Linux development window, not the product. The macOS menu bar
  item is unverified until a milestone check on a real Mac.
- To compare pixels in a test instead of looking, write a screenshot test: see
  `docs/areas/ui.md` and the `update-screenshots` skill.
