# zapros

An SQL IDE for Linux in Rust (egui) for **PostgreSQL**, **ClickHouse**,
**Oracle** and **SQLite**.

- **Never freezes on the network.** Each connection runs on its own threads:
  one session for scripts, one for the object tree and hints. Slow links,
  dropped connections and long queries never block the window; a lost
  link is re-opened on the next request; a query can be stopped (PG cancel
  request, ClickHouse `KILL QUERY`, Oracle break, SQLite interrupt), and one
  the server won't stop can be abandoned (its session is made anew).
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
  the dialect, snippets (`sel`, `case`, `cte`, …).
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
  configurable). PL/SQL units and `$$` bodies are left as written.
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
- **Right-click menu** in the editor: run, explain, cut / copy / paste,
  select all, format, comment, duplicate, upper / lower case.
- **Results**: a tab per statement, virtual scrolling, sort by column,
  filter, copy cell / row / row as JSON / all as TSV. Rows load in batches
  (500 by default, "rows per batch" in the settings or per connection): the
  last statement's cursor stays open, the bar above the table loads the next
  batch, all the rest, or stops (closing the cursor). Export to CSV / TSV /
  JSON / SQL INSERT / Markdown / HTML / XLSX, whole or split into files of
  N rows, optionally loading every row first. A column header's right-click
  menu gives its totals over the rows shown: count, NULLs, sum, min, max,
  average. Messages (timings, errors; the server's error position is marked
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
