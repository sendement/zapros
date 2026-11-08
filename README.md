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
  default connection) and, under them, the workspace's scripts. Tabs and
  their text live in the session and come back on start.
- **Object tree** per connection: schemas / databases, tables, views,
  materialized views, functions, procedures, packages, sequences, triggers,
  types, synonyms, dictionaries, jobs (Oracle Scheduler, pg_cron); columns,
  keys, indexes. Double-click opens an object's DDL; "first 200 rows",
  "insert SELECT with all columns", copy / insert names.
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
- **Formatter** ("river" style, leading commas; keyword / function case
  configurable). PL/SQL units and `$$` bodies are left as written.
- **Highlighting** with a palette per theme (colour pickers, bold keywords,
  italic comments, brackets coloured by depth).
- **Results**: a tab per statement, virtual scrolling, sort by column,
  filter, copy cell / row / row as JSON / all as TSV, save as CSV;
  messages with timings, `RAISE NOTICE` and `DBMS_OUTPUT`; the server's
  error position is marked in the script.
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
| Ctrl+Shift+F | format |
| Ctrl+Space | hints |
| Ctrl+/ · Ctrl+D | comment lines · duplicate line |
| Ctrl+T · Ctrl+W · Ctrl+Tab | new · close · next tab |
| Ctrl+S | save to file |

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
