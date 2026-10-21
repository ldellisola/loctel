This is Milestone 1: the headless core, MCP, the menu bar app and the release
pipeline. Nobody has seen any of the Mac side run, so this list is the first
look. Try each item on a real Mac (macOS 26, Apple Silicon), compare with what
"right" says, and report the results in a new chat, the items that pass too.
That session records the findings in `docs/decisions/` (or as GitHub issues)
and fixes what is wrong. Small oddities are worth reporting.

1. **Install and first launch.** Download the `.dmg` of this release and open
   it. Drag `loctel` onto the `Applications` shortcut, then open `loctel` from
   Applications. macOS blocks it: open System Settings, Privacy & Security,
   click **Open Anyway** next to the message about `loctel`, and confirm.
   *Right:* loctel starts with an icon in the menu bar (three small circles
   joined by two lines), sharp on the Retina screen, dark on a light menu bar
   and light on a dark one. There is **no Dock icon**, not even a flash of
   one at launch, and loctel is not in the ⌘-Tab switcher.
2. **The menu.** Click the menu bar icon.
   *Right:* the first row is greyed out and reads
   `Receiving · 0 spans/s · 0 logs/s`. Then, in this order: `Copy MCP address`,
   `Pause receiving`, a line, `Start at login`, a line, `Quit loctel`. Choose
   `Copy MCP address` and paste somewhere: you get
   `http://127.0.0.1:4319/mcp`. (There is no ⌘Q in the menu; say if you miss
   it.)
3. **A service on the Mac.** Start any service that exports OpenTelemetry
   (traces or logs) with `OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317`
   (gRPC) or `http://localhost:4318` (HTTP), and make some requests. Open the
   menu and keep it open for ten seconds while the service is busy.
   *Right:* the header shows non-zero rates, such as
   `Receiving · 12 spans/s · 3 logs/s`. The open menu does not flicker or
   close by itself. If the header does not change until you reopen the menu,
   say so. (No service handy? From a checkout of this repository, `cargo run
   -p loctel-meshgen -- --scenario checkout-failure --transport http-json
   --endpoint http://127.0.0.1:4318 --repeat 30` sends a failing checkout
   every second for half a minute: the header shows about `15 spans/s` and
   `7 logs/s`.)
4. **A Docker container.** Run a container that exports OpenTelemetry with
   `OTEL_EXPORTER_OTLP_ENDPOINT=http://host.docker.internal:4317` (Docker
   Desktop for Mac), and make it do some work.
   *Right:* the header shows rates, as in item 3. If it does not, **report
   it**: loctel then has to listen on all interfaces and not only on
   loopback (design §4).
5. **An agent asks why a request failed.** Run
   `claude mcp add --transport http loctel http://127.0.0.1:4319/mcp`. Start
   Claude Code, make a request to your service fail, and ask Claude "why did
   my last request fail?".
   *Right:* Claude calls loctel's tools and answers with the name of the
   service the request failed in and the exception. (No failing service
   handy? Send the fake one with the command in item 3, without `--repeat`:
   Claude should say the checkout failed in `payments-api`, with a
   `CardGatewayTimeoutException`.)
6. **Pause and Resume.** Choose `Pause receiving` while a service is sending.
   *Right:* the row becomes `Resume receiving`, the header reads `Paused`,
   the service sees no errors (loctel still answers success), and nothing new
   arrives. Choose `Resume receiving`: the header goes back to
   `Receiving · ...` and the rates return.
7. **Start at login.** The first launch turns it on.
   *Right:* `Start at login` has a checkmark (macOS may have shown a
   "background item added" notice), and System Settings, General, Login Items
   & Extensions lists loctel as on. Log out and log in again: loctel is
   running, with its icon in the menu bar. Switch it off from the menu and the
   checkmark goes; switch it off in System Settings and the checkmark follows
   the next time you open the menu.
8. **Quit.** Choose `Quit loctel`.
   *Right:* the icon disappears and the process is gone (`pgrep -x loctel`
   prints nothing). The ports are free again: `lsof -nP -iTCP:4317 -iTCP:4318
   -iTCP:4319 -sTCP:LISTEN` prints nothing.
