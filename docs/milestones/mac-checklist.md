Each section below is one milestone whose Mac side nobody has confirmed yet;
a section leaves this list once you have. Do Milestone 1 first: Milestones 2
to 5 build on a loctel that is installed (Milestone 1, item 1), and Milestone 2
on one that Claude Code is connected to (Milestone 1, items 1 and 5). This
release is Milestone 5's, which is the last of the plan, so **the menu of
Milestones 1 to 3 is gone** (it went in Milestone 4): a click on the menu bar icon
opens the popover (Milestone 4, items 1 and 2), and the items of the earlier
sections that say "the menu" mean its rows (the note at the start of Milestone 1
maps them). There is no `Edit discard rules…` any more (Milestone 2's text that
mentions it: open `rules.toml` in an editor yourself, or use the Discard rules
screen, Milestone 4, item 4). Loctel shows in the Dock and in ⌘-Tab for as long
as its main window or its Settings window is open (with neither open it still has
no Dock icon, as Milestone 1, item 1 says). Milestone 5 added the disk cap to
Settings and made the popover's fourth tile the disk, and the Milestone 4 text
below already says what is there now. Those are not differences to report.

### Milestone 1

This is Milestone 1: the headless core, MCP, the menu bar app and the release
pipeline. Nobody has seen any of the Mac side run, so this list is the first
look. Try each item on a real Mac (macOS 26, Apple Silicon), compare with what
"right" says, and report the results in a new chat, the items that pass too.
That session records the findings in `docs/decisions/` (or as GitHub issues)
and fixes what is wrong. Small oddities are worth reporting.

**A note for v0.1.3 and later:** since Milestone 4 the menu that this section
describes is the **popover** (Milestone 4, item 1 says what it looks like). A
tester on v0.1.3 or later uses the popover for every item below that says "the
menu":

- the first row, `Receiving · 12 spans/s · 3 logs/s`, is the pill (`Receiving`,
  `Paused` or `Port problem`) next to the title, and the rates are the first two
  of the four tiles (`spans per second` and `log lines per second`), which
  refresh every second while the popover is open;
- `Copy MCP address` is the `Copy` button at the end of the MCP row, and it still
  puts `http://127.0.0.1:4319/mcp` on the clipboard;
- `Pause receiving` / `Resume receiving` and `Quit loctel` keep their names, and
  `Start at login` is a switch (on means the checkmark);
- ⌘Q now quits from the popover (item 2 says the menu had none; that no longer
  holds);
- the popover closes when you click anywhere else, so item 3's "keep the menu open
  for ten seconds" is "keep the popover open", and its "does not close by itself"
  still stands;
- the rows are not in the order the items list: Milestone 4, item 1 gives the
  popover's order, which is the one to compare with.

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
   is, and the rules row shows a non-zero percentage. The percentage counts
   everything since loctel started, so traffic from earlier checks (such as
   Milestone 1's) keeps it below 100%, and with a busy service of your own it
   can still round to 0% if health checks are rare.
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

### Milestone 3

This is Milestone 3: the main window. The window, its screens and its colours
are tested on Linux with software rendering; what nobody has seen is any of it
on a Mac: the window opening from the menu bar, loctel in the Dock while it is
open, Apple's Liquid Glass behind the sidebar and the toolbar, and the trackpad
and the fast scrolling. Some things were not in Milestone 3's own release and are
in this one (Milestone 4): the sidebar has `Logs` and `Discard rules` besides
`Map & traces`, and the span logs under the waterfall have an `Open in Logs`
button; those are Milestone 4's items. The text is the bundled Inter font, not
San Francisco (say if it looks out of place). Items 4 to 8 need data: from a
checkout of this repository, the commands in Milestone 1, item 3 send it, and
each item says which scenario to use. Report what you see, and say which item
it is.

1. **Open loctel.** Click the menu bar icon and choose **Open loctel**. It is
   the first row after the two greyed-out header rows, followed by a line, and
   the order is: status row, rules row, `Open loctel`, a line,
   `Copy MCP address`, `Pause receiving`, `Edit discard rules…`, a line,
   `Start at login`, a line, `Quit loctel`.
   *Right:* the window appears in under half a second, and loctel now shows in
   the Dock and in the ⌘-Tab switcher. Choose `Open loctel` again with the window
   open: the same window comes to the front, not a second one. Close the window
   (the red button): the Dock icon and the ⌘-Tab entry go, and loctel keeps
   running in the menu bar (the menu still answers and the endpoints still
   listen). Open it again, then click the Dock icon while the window is open:
   it comes to the front; minimise the window and click the Dock icon: it comes
   back. Open and close the window ten times in a row: one window each time and
   no stray Dock icon. (⌘O brings the window to the front too, but only while
   a loctel window has the keyboard: the menu shows no shortcut and does not
   answer ⌘O, which is expected and not a difference to report.)
2. **Glass.** Look at the sidebar (left) and the toolbar (the strip across the
   top of the content), and drag the window over different backgrounds, such as
   a photo desktop and a white window.
   *Right:* the sidebar and the toolbar show what is behind them through
   Apple's glass (blurred and refracted, changing as you move the window over
   different backgrounds). The content areas (the map, the table, the trace
   view) are solid, and so are the gaps between the panels. Report whether the
   glass looks like Liquid Glass or like plain blur; plain blur is the fallback
   loctel uses when it cannot attach the glass, and it prints one line about it
   to stderr when started from a terminal. Also say if you see a seam where the
   glass meets the panel's rounded corners, or if the window ever hangs while
   you resize it quickly.
3. **Light and dark.** With the window open, switch the appearance in System
   Settings, Appearance.
   *Right:* the window follows within a second, with no restart: dark panels
   and light text on the dark look, white panels and dark text on the light
   one, and the text stays readable on the glass in both.
4. **A live map and table.** Send the failing checkout on repeat: `cargo run
   -p loctel-meshgen -- --scenario checkout-failure --transport http-json
   --endpoint http://127.0.0.1:4318 --repeat 120 --interval-ms 500` (a minute
   of traffic). Look at the home screen.
   *Right:* the map shows `web-bff` to `orders-api` to `payments-api` (and the
   other services of the checkout) with `payments-api` red, and the lines leading
   to it (the services above it, which see the error come back, are red too),
   and a request rate on each node. In the table below, new rows appear at
   the top about twice a second, each with `payments-api · POST card-gateway +1`
   in red under `Failed in`, without the scroll jumping: the table does not
   shake or flicker as rows arrive. The status box at the bottom of the sidebar
   shows non-zero rates. Then scroll the table down a little while the traffic
   goes on and say what the rows you were looking at do: loctel keeps the scroll
   position and does not hold your place against rows arriving above it, so they
   drift down, and it is worth knowing whether that is tolerable (item 8 has
   the Pause button for reading a moving table).
5. **Filtering.** Click `payments-api` on the map.
   *Right:* the node gets a blue ring, the query bar fills in with
   `{ resource.service.name = "payments-api" }` and the table shows only traces
   that went through it. Click it again (or empty space on the map): the ring
   and the filter go. Then click in the query bar and type a broken query such
   as `{ status = `: *right:* a red line appears under the bar saying
   `at column 12: expected a value`, and the rows stay (the table does not go
   empty). Fix the query (`{ status = error }`) and the line goes and the table
   shows the traces that have an erroring span. Try the `Errors only` and
   `Slower than 1 s` toggles too.
6. **The trace view, and the map with a trackpad.** Click a failed checkout in
   the table.
   *Right:* the trace opens with a red badge `Failed in payments-api · POST
   card-gateway +1`, the waterfall of 15 spans with the failed-in span (a
   `POST` from `payments-api` to `card-gateway`, 236 ms in) selected, and the
   inspector on the right showing it with `Error · timeout`. Click the row
   `POST /charges` above it: the inspector shows an exception card with
   `Payments.Api.Gateway.CardGatewayTimeoutException` and its stack trace, and
   the four logs of the span appear under the waterfall. Press Esc: you are back
   on the home screen with your query and rows as you left them. Now use the
   trackpad on the map: pinch to zoom, two-finger drag to pan (and
   ⌘-scroll to zoom, a drag with the pointer to pan, `Fit` in the top right to go
   back). *Right:* the map zooms around the fingers and pans smoothly, with no
   jump when you let go.
7. **A big trace.** Send the 100,000-span trace: `cargo run -p loctel-meshgen
   -- --scenario huge-trace --transport http-json --endpoint
   http://127.0.0.1:4318`. Wait a few seconds for it to arrive, find the row
   `POST /api/bulk` in the table (it has 100,000 spans) and open it, then scroll
   the waterfall fast from top to bottom with the trackpad (and drag the scroll
   bar).
   *Right:* it stays smooth, with no blank rows while you flick and no beachball.
   Report any stutter, how long the trace takes to open, and whether the window
   stayed responsive.
8. **The toolbar.** With the traffic of item 4 running, look at the right end of
   the toolbar: a `Live` pill with a green dot, `Last 15 minutes` with a caret,
   `Pause` and `Clear all`. Click `Pause`.
   *Right:* the pill turns amber and says `Paused`, the button becomes a blue
   `Resume`, and the table and the map stop changing (the numbers on the nodes,
   the rows) while the status box in the sidebar still shows traffic arriving.
   Click `Resume`: they catch up at once and run again. Click `Last 15 minutes`:
   a menu of `Last 5 minutes`, `Last 15 minutes`, `Last hour` and `Everything`
   opens under it, over the map; choose `Last 5 minutes` and the menu closes and
   the button says so; open it again and click anywhere else: it closes and
   nothing changes. Finally click `Clear all`: the map and the table empty at
   once (there is no confirmation), and new traces show up again within a
   second. Report whether the buttons look right on the glass (they are solid,
   not glass) and whether every click lands where you aimed it.

### Milestone 4

This is Milestone 4: the Logs screen, the Discard rules screen, the menu bar
popover (which replaces the menu of Milestones 1 to 3) and the Settings window.
All of it is tested on Linux with software rendering, with screenshots of every
screen in both themes; what nobody has seen is any of it on a Mac: where the
popover opens and how it closes, Apple's glass behind it, the keys in a window of
a menu bar app that has no menu, and the Settings window and the login switch.
The text is the bundled Inter font, not San Francisco, as in Milestone 3. There
is no `Edit discard rules…` row any more: the rules are edited in the Discard rules
screen (item 4), or by hand in `~/Library/Application Support/loctel/rules.toml`.
The disk cap went into Settings, and the popover's fourth tile became the disk, in
Milestone 5 (the text below says what is there now; Milestone 5's own list checks
them). Items 3 to 5 need data: from a
checkout of this repository, the commands in Milestone 1, item 3 send it, and each
item says which scenario to use. Report what you see, and say which item it is.

1. **The popover.** Click the menu bar icon.
   *Right:* a glass panel, 336 points wide with rounded corners, opens just under
   the icon (a few points below the menu bar), centred on where you clicked, and
   kept inside the screen if the icon is near its edge. Top to bottom it has: the
   title `loctel` with a pill (`Receiving`, green); four tiles (`spans per second`,
   `log lines per second`, `traces held`, and the disk: `0 B` over `on disk, cap
   4 GB`); the three endpoints
   (OTLP gRPC `127.0.0.1:4317`, OTLP HTTP `127.0.0.1:4318`, MCP `127.0.0.1:4319/mcp`
   with a `Copy` button) with green dots; the rules line (`No discard rules on`);
   a line; `Open loctel` ⌘O, `Pause receiving`, `Clear all data…`, `Start at login`
   with a switch; `Settings…` ⌘,; a line; `Quit loctel` ⌘Q. Clicking anywhere else
   (the desktop, another app, another menu bar item) closes it. Esc closes it. Click
   the icon again while it is open: it closes and does not open again. Open it and
   press ⌘Q: loctel quits (the icon goes and `pgrep -x loctel` prints nothing).
   Say whether the glass looks like Liquid Glass or like plain blur, whether there
   is a square corner or a shadow behind the round panel, and, with a second
   display or the Dock at a side or an icon near the right edge, whether it still
   opens under the icon.
2. **The popover actions.** Open the popover again and try each action. `Pause
   receiving` becomes `Resume receiving` and the pill `Paused` at once (the
   panel stays open); send some traffic (Milestone 1, item 3) and see that nothing
   arrives, then `Resume receiving`. `Copy` beside MCP puts
   `http://127.0.0.1:4319/mcp` on the clipboard (paste it). `Clear all data…` asks
   `Clear all traces and logs?` first: `Cancel` puts the row back, and `Clear` empties
   the data (the `traces held` tile goes to 0). The rules line
   opens the main window on Discard rules. `Open loctel` opens the window on
   Map & traces, and the popover closes. `Settings…` opens the Settings window.
   *Right* for the last three: the panel closes, and loctel is in the Dock and ⌘-Tab
   while a window is open.
3. **Logs.** With the failing checkout once and then a log flood on repeat (`cargo
   run -p loctel-meshgen -- --scenario checkout-failure --transport http-json
   --endpoint http://127.0.0.1:4318`, then `cargo run -p loctel-meshgen --
   --scenario log-flood --size 500 --transport http-json --endpoint
   http://127.0.0.1:4318 --repeat 120 --interval-ms 500`), open Logs: ⌘2 or the
   sidebar. *Right:* new lines appear at the top of the list about twice a second
   and the list does not jump; the histogram above it grows its last bars; the
   `Live tail` pill in the toolbar is green. Scroll down the list a little: the tail
   stops (the pill goes amber) and a `New lines · Jump to newest` pill appears over
   the list, and the lines you are looking at stay put while more arrive. Click the
   pill (or scroll back to the top): the tail resumes. Type
   `{service_name="payments-api"} | level = "error"` in the search bar: the list
   shows the checkout's two errors from `payments-api` and nothing else, and the
   counts and the histogram follow. Type something broken (`{service_name=`): a red
   line says where, and the list keeps its rows. Click `ERR`, `WRN`, `INF` and `DBG`
   to turn levels off and on. Click the error line to expand it: the whole body (the
   stack trace on several lines), the labels and three buttons; `Open trace` opens
   the trace view with that log's span selected, and Esc comes back to Logs with the
   query and the open line as you left them. `Show lines around this one` shows the
   neighbouring lines with `Back to search`. Try ⌘1, ⌘2 and ⌘3 from the trace view
   and from an input.
4. **Discard rules** (⌘3). Send health checks (`--scenario health-checks`, from
   Milestone 2, item 2) a few times, then press `New rule`. Name it, choose
   `Traces (TraceQL)`, type the query `{ span.url.path = "/health" }` (adjust to your
   data) and watch the box under it: `Tried on the last 5 minutes` and what the rule
   would have dropped, which follows your typing about a third of a second after you
   stop. Break the query: a red `at column N: ...` line appears and the preview asks
   you to fix it. Fix it and `Save`: a card appears with a switch, the name, a
   `TRACEQL` chip and the query. Send health checks again: the card's count (`N
   spans dropped in M traces`) grows and the header says how much of the incoming
   data is dropped. Toggle the switch off (`Off`, and the checks arrive again) and on
   again. Click the card to open it in the editor and type something into its name; with
   it open, edit `rules.toml` in a text editor (change the name of a rule there)
   and save the file: within a few seconds the amber banner `The rules file changed
   while you were editing.` appears with `Keep my draft` and `Discard my draft`, your
   typing stays, and `Save` waits for your choice. Try
   `Delete` (it asks first) and `Cancel`. Check that the popover's rules line agrees
   with the screen.
5. **"Discard lines like this…"** From Logs, expand any line (a flood line will
   do) and click `Discard lines like this…`. *Right:* the Discard rules screen
   opens with the editor on a new, unsaved rule: the name `Lines like "<the start
   of the message>…"`, `Logs (LogQL)` chosen, and the query `{service_name="..."} |=
   "<the message>"`, and the preview shows how many of the last five minutes' lines
   it would have dropped. `Cancel` leaves nothing behind (check `rules.toml`).
6. **Settings.** Choose `Settings…` in the popover (or ⌘, in the main window). *Right:*
   a window 520 by 480 points (360 before Milestone 5 added `Storage`) that cannot be
   resized, with the three ports (4317, 4318, 4319) each with a green dot and
   `listening`, `Storage` with the `Disk cap` field (`4` GB, and `Using ... of 4 GB`
   under it), a `Start loctel at login` switch and `Apply` and `Close`; choosing `Settings…` again brings the same window
   forward. Change the OTLP HTTP port to a free one (such as 4400) and `Apply`: the
   row stays `listening` and a service pointed at the new port arrives (send meshgen
   to `http://127.0.0.1:4400`; the old port 4318 is free again). Change it to a port
   that is in use (start `python3 -m http.server 4500 --bind 127.0.0.1` first): the
   row turns red and says `failed: address in use`, `otlp-http: address in use`
   appears by the buttons, the popover shows `Port problem`, the other two
   endpoints keep working, and the setting is kept. Give two endpoints the same
   port, or a port such as 70000: the window refuses before applying, in words under
   the field. Change the port back. Quit and start loctel: the ports are the ones you
   left.
7. **Start at login** from the popover and from Settings agree. Switch it off in
   the popover: Settings shows it off (close and open it again if it is open) and
   System Settings, General, Login Items & Extensions shows loctel off. Switch it on
   in Settings: the popover shows it on. Switch it off in System Settings: the next
   time the popover or Settings opens it shows off. (Milestone 1, item 7 has the
   rest: that it is on after the first launch, and that a login starts loctel.)

### Milestone 5

This is Milestone 5: scale. Loctel moves the traces and the log lines it no longer
needs in memory into temporary files (in a folder of its own under `$TMPDIR`, deleted
when loctel quits), keeps those files under a disk cap (4 GB unless you change it in
Settings), scrolls the Logs list through millions of lines, and holds itself to
performance budgets that CI checks on Linux. All of it is tested on Linux, and the
budgets are measured there (a position in ten million log lines in under 20 ms, a
100,000-span trace opening in under 250 ms, the headless start in under 300 ms), and
both themes pass a contrast check by arithmetic. What nobody has seen is any of it on a
Mac: the start, a big trace and a big Logs list on Apple Silicon and Metal, the files and
the memory on APFS, and the light theme next to Apple's glass. Items 2 to 5 need data:
from a checkout of this repository, the commands in Milestone 1, item 3 send it, and each
item gives its command (the Linux numbers say a flood of two million lines goes in about
twenty seconds). Report what you see, and say which item it is, the items that pass too.

1. **Startup.** Choose `Quit loctel` in the popover (or ⌘Q with it open), wait a second
   and open loctel again from Applications. *Right:* the icon is in the menu bar at once,
   with no pause before it. Click it and choose `Open loctel` (⌘O): the window is on
   screen in under half a second. This one is by feel; if you can, say how long it was
   (a stopwatch on a screen recording will do), and whether the window drew its first
   frame at once or showed a blank one first.
2. **A big trace.** Send the 100,000-span trace: `cargo run -p loctel-meshgen --
   --scenario huge-trace --transport http-json --endpoint http://127.0.0.1:4318`. Find the
   row `POST /api/bulk` in the table (it has 100,000 spans) and open it, then scroll the
   waterfall from top to bottom as fast as you can (a flick with the trackpad, the
   scroll bar dragged, and the same back up). *Right:* it stays smooth, with no blank
   rows while you scroll and no beachball. Press Esc, then **wait a minute** with loctel
   running: the trace has been complete for ten seconds, and half a minute later it
   moves to a file (item 4 says where to look). Open it again from the table. *Right:*
   it opens just as fast as the first time, and looks the same. Report how long the
   open took each time.
3. **Many logs.** Quit loctel and open it again (or `Clear all data…` in the popover), so
   that nothing else is held. Send two million log lines: `cargo run -p loctel-meshgen --
   --scenario log-flood --size 2000000 --transport http-protobuf --endpoint
   http://127.0.0.1:4318` (about twenty seconds). Open Logs (⌘2). *Right:* the footer
   reads `2,000,000 lines · newest first`. Drag the scroll bar at the right edge of the
   list (a thin track with a thumb) from the top to the bottom and back, and flick the
   list with the trackpad. *Right:* rows appear as you go, with no stall longer than a
   blink and no empty stretch left on the screen when you stop; the thumb follows the
   list; at the bottom are the oldest lines of the flood. Click in the search
   bar and type `{service_name="payments-api"} |= "flushed"` and press Return. *Right:*
   the list shows only `payments-api` lines saying `flushed N records`, and the first rows
   and the count appear within a second. Clear the query (the list is the full one
   again) and click `ERR`, `WRN` and `DBG` off and on: each change shows its rows and a new
   count within a second.
4. **The disk.** With the lines of item 3 in:
   - Click the menu bar icon. *Right:* the fourth tile is the disk: a size such as
     `37 MB` over `on disk, cap 4 GB` (two million of these lines compress to about
     forty MB). Send the flood again and watch the popover (it refreshes every second
     while open): the tile grows, in steps, as the lines are written to the files in chunks.
   - In Terminal, `open "$TMPDIR/loctel"` shows the folder in Finder. *Right:* one
     folder named `run-<number>-<number>` with `logs-000001.seg`-style files (and
     `traces-…seg` ones after item 2) of up to 64 MB each. The same files are in `ls -la
     "$TMPDIR"/loctel/run-*`.
   - Open Settings (⌘,). *Right:* under `Storage`, the `Disk cap` field reads `4` GB and
     the line under it `Using … of 4 GB`, with the size from the tile. Type `0`, then
     `101`: the message `Enter a size from 1 to 100 GB` appears under the field and `Apply`
     refuses. Type `1` and press `Apply`: the line under the field says `of 1 GB` and the
     popover's tile `on disk, cap 1 GB`. Put it back to `4` and `Apply`.
   - To see the cap delete data (the files here are small, so 1 GB is far away), quit
     loctel, open `~/Library/Application Support/loctel/settings.toml` in a text editor,
     set `disk_cap_bytes = 50000000` (add the line if it is not there), save and open
     loctel. Send the flood twice more. *Right:* the tile never stays above about 50 MB
     (it can go over for a moment, and the oldest file goes whole, so it drops in steps),
     the Logs footer says fewer lines than you sent, and loctel stays responsive. Quit
     loctel, delete the `disk_cap_bytes` line (or set it back to `4294967296`) and open
     it again.
   - Quit loctel (`Quit loctel`). *Right:* the `run-…` folder is gone from `$TMPDIR/loctel`
     (the `loctel` folder itself stays). Open loctel, and in Terminal force quit it with
     `pkill -9 -x loctel`: the `run-…` folder stays. Open loctel again: the old folder is
     deleted at once and a new one is there.
5. **Memory.** Open Activity Monitor (Memory tab) and find `loctel`. Send a steady stream
   for ten minutes, in two Terminal windows: `cargo run -p loctel-meshgen -- --scenario
   log-flood --size 10000 --transport http-protobuf --endpoint http://127.0.0.1:4318
   --repeat 600 --interval-ms 1000` and `cargo run -p loctel-meshgen -- --scenario
   checkout-failure --transport http-json --endpoint http://127.0.0.1:4318 --repeat 600
   --interval-ms 1000`. Write down loctel's `Memory` at the start and after 2, 5 and 10
   minutes. *Right:* it rises at first and then levels off (the Linux budget for ten
   million lines and a million spans is 1.5 GB, so a few hundred MB here is expected); it
   does not keep climbing the whole ten minutes. Say what the numbers were, and whether
   the window stayed responsive and the popover's tile kept growing.
6. **The light theme.** In System Settings, Appearance, choose Light (loctel follows).
   Look over every screen: Map & traces, a trace (open one from the table), Logs (with a
   line expanded), Discard rules (with its editor open), the Settings window and the
   popover. *Right:* all text is readable, including the grey secondary text (the
   second line of a table row, the labels under the popover's tiles and the hints under
   Settings' fields), the hairlines between rows are visible, and
   the red, amber and blue words (a failed trace, `Paused`, a link) read clearly on the
   panels. Report anything faint, with the screen it is on. Then choose Dark again and
   look at the same screens once: the lines between rows are a little lighter than in
   Milestone 4, and that is meant.
