Each section below is one milestone whose Mac side nobody has confirmed yet;
a section leaves this list once you have. Do Milestone 1 first: Milestone 2
builds on a loctel that is installed and that Claude Code is connected to
(Milestone 1, items 1 and 5). This release is Milestone 2's, so its menu has
two more rows than Milestone 1, item 2 lists (the rules row and **Edit discard
rules…**, see Milestone 2, item 1): that is not a difference to report.

### Milestone 1

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

### Milestone 2

This is Milestone 2: TraceQL and LogQL search for agents, and discard rules
that drop noise (health checks, say) as it arrives. The search runs the same
on Linux; what nobody has seen is the new menu (the rules line and
**Edit discard rules…**) and Claude Code using the new `query` arguments.
loctel never discards anything you did not ask it to: its rules file starts
with no rules. The file is `~/Library/Application Support/loctel/rules.toml`
(its folder is next to `settings.toml`). Report what you see, and say which
item it is.

1. **The rules line and the rules file.** Click the menu bar icon.
   *Right:* under the status row there is a second greyed-out row reading
   `No discard rules on`, and the order is: status row, rules row, `Copy MCP
   address`, `Pause receiving`, `Edit discard rules…`, a line, `Start at
   login`, a line, `Quit loctel`. Choose **Edit discard rules…**:
   `rules.toml` opens in a text editor (which one is macOS's choice; say which
   it was). It holds no rules, only an explanation of the format and a
   health-check rule shown as a comment (every line starting with `# `).
2. **A rule that silences health checks.** In `rules.toml`, write your own rule,
   or take the commented-out example, remove the `# ` from its six lines, adjust
   its paths if your service names its health route differently, and save.
   *Right:* within about 5 seconds the rules row reads
   `1 discard rule on · 0% dropped` (open the menu again if it is already open).
   Now send health checks: from your service, or from a checkout of this
   repository, `cargo run -p loctel-meshgen -- --scenario health-checks
   --transport http-json --endpoint http://127.0.0.1:4318`. Ask Claude, in a
   session connected to loctel (Milestone 1, item 5), to list the recent traces
   through loctel. *Right:* no `/health` trace is listed, other traffic still
   is, and the rules row shows a non-zero percentage (with only meshgen's health
   checks it reads 100%; with a busy service of your own it is smaller, and
   can still round to 0% if health checks are rare).
3. **Turning the rule off.** Set `enabled = false` in the rule and save.
   *Right:* within about 5 seconds the rules row reads `No discard rules on`,
   and the health checks sent from now on appear when you ask Claude for recent
   traces again (the ones dropped earlier stay gone: dropped data is not kept).
   Set `enabled = true` and save to turn it on again.
4. **A mistake in the file.** Break the query: remove one of the double quotes
   inside it, and save. *Right:* the rules row reads
   `1 discard rule on · 1 has an error`, the rule drops nothing, and nothing
   crashes or closes. Fix the quote and save: the row is back to
   `1 discard rule on · 0% dropped` (or the percentage). Then break the file
   itself instead (remove the single quote that ends the `query` line): the row
   reads `Discard rules file has an error`, and loctel keeps applying the
   rules it had before the mistake. Fix it again and the row recovers.
5. **A TraceQL query from Grafana.** Ask Claude to run, through loctel, a query
   you use in Tempo, such as
   `{ resource.service.name = "<yours>" } >> { status = error }`.
   *Right:* Claude passes it as the `query` of `search_traces`, and the traces
   it lists make sense: they contain an erroring span below a span of your
   service, and the answer names the spans that matched. (No service handy?
   Send the fake checkout first, with the command in Milestone 1, item 3 and
   without `--repeat`, and use `web-bff` as the service: Claude should find the
   failed checkout and its erroring spans in `payments-api`.) Ask for one that
   loctel does not support, such as one ending in `| select(span.foo)`: Claude
   reports an error that says it is not supported yet, and where in the query.
6. **A LogQL query.** Ask Claude for logs with a LogQL query, such as
   `{service_name="<yours>"} | level = "error"`.
   *Right:* it passes it as the `query` of `search_logs`, and the logs listed
   are your service's errors and nothing else. (With the fake checkout:
   `{service_name="payments-api"} | level = "error"` gives two logs, the card
   gateway timeout and the charge failure.)
