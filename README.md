# zapros

An SQL IDE for Linux in Rust (egui) for **PostgreSQL**, **ClickHouse**,
**Oracle** and **SQLite**.

- **Never freezes on the network.** Each connection runs on its own threads:
  one session for scripts, one for the object tree and hints. Slow links,
  dropped connections and long queries never block the window; a lost
  link is re-opened on the next request; a query can be stopped (PG cancel
  request, ClickHouse `KILL QUERY`, Oracle break, SQLite interrupt), and one
  the server won't stop can be abandoned (its session is made anew). A
  ClickHouse session still busy a few seconds on (SESSION_IS_LOCKED) gives
  way to a new one, with a note in Messages. A window the compositor
  doesn't show (another workspace or screen) no longer turns "not
  responding": no vsync wait, frames paced by hand.
- **Two rows of tabs.** Workspaces (named, coloured, each with its own
  default connection) are tabs over a sheet whose outline, in the
  workspace's colour, runs on from the active tab; on the sheet, the
  workspace's scripts, the editor and the results. Tabs and their text
  live in the session and come back on start. The side panel runs the
  full height, its top row holding the panel switch (objects / files) and
  the menus as icons.
- **Transactions** (PostgreSQL, Oracle, SQLite): auto-commit per
  connection (off by default), switchable from the toolbar. A SQLite
  transaction that only read is ended at once. With it off, changes gather
  until Commit or Roll back; the toolbar and status line show when some
  are pending, and switching auto-commit on, disconnecting or closing the
  window asks what to do with them. In PostgreSQL each statement runs under
  a savepoint, so one that fails is undone alone and the transaction goes
  on. ClickHouse has no transactions: the controls hide.
- **Object tree** per connection: schemas / databases, tables, views,
  materialized views, functions, procedures, packages, sequences, triggers,
  types, synonyms, dictionaries, jobs (Oracle Scheduler, pg_cron); columns,
  keys, indexes. Double-click opens an object's DDL; "first 200 rows",
  "insert SELECT with all columns", copy / insert names.
- **Packages in two parts.** An Oracle package (opened from the tree or a
  file) shows its specification and body as two sub-tabs (Alt+1 / Alt+2),
  each run (compiled) on its own and saved together.
- **Structure panel** (Ctrl+Shift+O) beside the editor: the script's
  statements and the objects they create; in a package its functions,
  procedures, types, subtypes, cursors, exceptions, constants, variables
  and initialization (both parts when split). A click goes there, a double
  click on a declaration goes to its body; the entity under the cursor is
  lit; a search narrows it.
- **Project files** in the same panel: a folder of `.sql` scripts.
- **Static analysis while typing**: unclosed strings, comments and
  brackets, `CASE` without `END` — and in PL/SQL and PL/pgSQL bodies
  `BEGIN` / `IF` / `LOOP` without their `END` — get a red background until
  fixed; stray commas, empty `WHERE` / `FROM`, an operator with nothing
  after it are errors; `UPDATE` / `DELETE` without `WHERE`, `JOIN` without
  `ON`, tables and `alias.columns` the database doesn't have are underlined.
  Hover for the reason. Running a script with errors asks first.
- **Hints**: columns after `alias.`, tables after `FROM` / `JOIN`, a join
  condition guessed from column names after `ON`, keywords and functions of
  the dialect, snippets (`sel`, `case`, `cte`, …). Schemas can be put in
  quotes always (a setting).
- **SQL inside strings.** Dynamic SQL (`EXECUTE IMMEDIATE`, PL/pgSQL
  `EXECUTE`, `OPEN … FOR`, `format()`, `DBMS_SQL.PARSE`), strings kept in
  variables that end up there, literals reading as a statement, and
  PostgreSQL `$$` bodies are coloured as SQL (strings in them on a faint
  wash). A statement built with `||` is read as one text, the expressions
  standing in as placeholders, so quotes, brackets and clauses pair up
  across the pieces. The formatter lays such a statement out — a `q'[…]'`
  or `'…'` literal over several lines, a `||` chain one piece per line with
  the quotes under each other — and only re-cases keywords in a piece of a
  statement (`v_sql := v_sql || ' and …'`); anything it can't redo token
  for token is left alone. With "split a string over several lines" off in
  the formatter's settings, strings keep their lines and only get the
  keywords' case.
- **Formatter** ("river" style, leading commas; keyword / function case
  configurable). PL/SQL units and `$$` bodies are left as written. In
  ClickHouse `default` naming a database, user or cluster
  (`default.orders`, `USE default`, `TO default`) keeps its case.
- **Highlighting** with a palette per theme (colour pickers, bold keywords,
  italic comments, brackets coloured by depth) and, as in listok, rainbow
  bands under the indents: one per construct a line is nested in —
  brackets, CASE…END, BEGIN / DECLARE / IS…END, IF, LOOP, and the clauses
  SELECT / FROM / JOIN / WHERE / GROUP BY…, inside SQL strings and `$$`
  bodies too. The block (bracket, CASE, BEGIN…END, IF, LOOP) at the caret
  gets a frame in its band's colour, the one under the pointer a fainter one.
- **Tabs**: a script tab's ✕ always shown or only under the pointer (the
  tab grows to it) — in the settings; toolbar buttons in their colours (run
  green, stop red, plan blue…).
- **Editor niceties**: folding (▾ / ▸ in the gutter beside a block or a
  statement over several lines; a folded one shows "⋯ n", a click or the
  caret going in opens it); the caret's line on a faint background (under
  the bands, which only change hue); the word picked, or under a caret
  resting a moment, marked wherever else it is; a bracket under the pointer
  (or picked) marked with its pair.
- **Notebooks**: any tab can be shown as a notebook (📓 on the toolbar):
  cells of SQL or Markdown, one under another, each SQL cell with results
  of its own (batches, totals, export — as a tab's). Outputs live only in
  memory; a cell's 📌 keeps its output in the session. A Markdown cell's
  header switches it between View and Edit (or double-click its text; Esc
  or Ctrl+Enter go back), several may be open at once. A cell that is only
  a `WITH` lends its CTEs to the cells below: before a cell runs, the CTEs
  it uses (and the ones those use) are put into its own `WITH` — one
  `WITH`, nothing it doesn't use; run alone, a WITH cell shows its last CTE.
  Saved as SQL with `-- %%` / `-- %% [markdown]` marks (such files open as
  notebooks); "To plain SQL" opens it as an ordinary script, as it is or
  with every statement standalone (CTEs put in). Cells fold to their first
  line (one by one or all), an output's height is dragged by its top edge
  (pulled up, as the results panel's) or by the grip under it
  (double-click: back to the usual).
- **Pointer**: a pointing hand over everything that clicks (buttons,
  arrows, tabs, the sort squares); scroll bars always shown.
- **Block selection**: Alt and the mouse pick a rectangle of lines and
  columns; typing, Backspace / Delete, paste (a line each when the counts
  match) act on every line of it, Ctrl+C / Ctrl+X take its columns; Esc,
  a move or a plain click end it. The wheel scrolls while a selection is
  being dragged.
- **Right-click menu** in the editor: run, explain, cut / copy / paste,
  select all, format, comment, duplicate, upper / lower case.
- **Results**: a tab per statement, virtual scrolling, sort by column,
  filter, copy cell / row / row as JSON / all as TSV. Rows load in batches
  (500 by default, "rows per batch" in the settings or per connection): the
  last statement's cursor stays open, the bar above the table loads the next
  batch, all the rest, or stops (closing the cursor). Export to CSV / TSV /
  JSON / SQL INSERT / Markdown / HTML / XLSX, whole or split into files of
  N rows, optionally loading every row first. Totals on demand: a column header's
  right-click menu has Σ Totals → sum / min / max / average / count, the
  value shows in a row of its own under the table (right click there for
  another or to remove it), worked out again only when the rows change.
  Sorting by several columns (the header's little square opens under the
  mouse into ▲ ▼ ✕; Shift adds a level, the menu too). Columns are picked
  by a click anywhere on their header (Ctrl / Shift for several)
  and moved by dragging or from the menu; the order is kept for the next
  runs giving the same columns, and export follows it. A clicked cell
  lights its row and its column's header, the cell itself deeper; rows are
  picked by their number (Ctrl / Shift for several; Ctrl+C copies them).
  The narrow number column stays pinned at the left when the table scrolls
  sideways. NULLs sit on a shade of their own. Messages (timings, errors; the server's error position is marked
  in the script) and Output (`RAISE NOTICE`, `DBMS_OUTPUT`) are tabs of
  their own.
- **Passwords** in the desktop keyring (Secret Service: GNOME Keyring,
  KWallet); without one, in a ChaCha20-Poly1305 encrypted file (0600).
  Saving the password is optional: otherwise it's asked on connect.

## Keys

| | |
|---|---|
| Ctrl+Enter | run the statement at the cursor (or the selection) |
| Ctrl+Shift+Enter, F5 | run the whole script |
| Ctrl+E, F7 | explain plan |
| Shift+Esc | stop |
| Ctrl+Alt+Enter · Ctrl+Alt+Backspace | commit · roll back |
| Ctrl+Shift+O | structure panel (and its search) |
| Alt+1 · Alt+2 | package specification · body |
| Ctrl+Shift+F | format |
| Ctrl+Space | hints |
| Ctrl+/ · Ctrl+D | comment lines · duplicate line |
| Ctrl+T · Ctrl+W · Ctrl+Tab | new · close · next tab |
| Ctrl+S | save to file |

## Demo data

`examples/seed/` fills a shop database (customers, products, orders with
items and payments, staff in departments; names in Russian) with views,
functions, a procedure, a trigger, a sequence, a materialized view and —
in Oracle — a package, an object type, a synonym and a scheduler job:

```sh
psql -U shop -d shop -f examples/seed/pg.sql                        # 300k orders, 900k items
sqlplus shop/shop@//host/FREEPDB1 @examples/seed/ora.sql            # ora_user.sql as SYSTEM first
clickhouse-client --multiquery < examples/seed/ch.sql               # 1M orders, 5M page views
python3 examples/seed/sqlite.py shop.db                              # 100k orders
```

## Building

```sh
cargo build --release
```

Oracle needs Oracle Instant Client at run time (`libclntsh.so`, e.g. AUR
`oracle-instantclient-basic`); build without it: `--no-default-features`.

Settings and connections: `~/.config/zapros`; session: `~/.local/share/zapros`.

## Tests

`cargo test` runs the SQL engine and the window offscreen against SQLite.
Against real servers:

```sh
ZAPROS_TEST_PG=localhost:5432:db:user:pass \
ZAPROS_TEST_CH=localhost:8123:default:user:pass \
ZAPROS_TEST_ORA=localhost:1521:FREEPDB1:user:pass cargo test
```

Pictures of the window: `ZAPROS_SHOT_DIR=/tmp/shots cargo test ui_tests`.

## License

MIT. The SQL formatter grew out of the one in
[listok](https://github.com/sendement) (same author, MIT).
