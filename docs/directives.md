# Script directives

Status: **implemented** (`src/script.rs`), except what §9 and §11 mark as
not yet: the dry run, the "how much is ahead" count, hints and hover.

ClickHouse has no procedural language of its own (unlike Oracle's PL/SQL or
PostgreSQL's PL/pgSQL), yet work there often needs loops: the same
`INSERT … SELECT` over tables split by number (`events_0` … `events_15`) or by
date (`events_20261101`), `OPTIMIZE` over every table that matches, waiting
for mutations to finish. Directives cover that without leaving SQL: variables,
lists, dictionaries, loops and a little output, written in comments, so the
script stays SQL — highlighting, hints, formatting and other tools all keep
working, and a tool that doesn't know directives just skips them.

```sql
-- @set day = '2026-11-01' | date yyyy-mm-dd

-- @for i in 0 .. 15
insert into agg_daily
select toDate(ts), count() from events_&i where toDate(ts) = :day group by 1;
-- @print('shard ', i, ' done')
-- @end for
```

## 1. Where directives go

A directive is a line comment whose text starts with `@`:

```sql
-- @set x = 1
--@set x = 1          -- the space after -- is optional
-- @set x = 1 -- why  -- after the directive, " -- " starts a comment again
```

One directive per line. A directive line holds nothing but the directive
(no SQL before it on the same line). Everything else in the script is SQL
and runs as usual; SQL between `@for` / `@while` and its `@end` is the loop's
body and runs on every pass.

Directives work in a tab's script and in notebook cells. A loop opened in a
cell ends in the same cell.

A script runs as one when it's run whole (Run script) or a part of it is
picked and run; Run statement (one statement under the cursor) runs that
statement alone, its variables asked for as before. In a notebook, once
any cell has a directive every cell runs as a script, the variables
shared down the notebook (and kept for the next runs of its cells).

A line counts as a directive only when its first word is one (`set`,
`var`, `for`, `while`, `if`, `elif`, `else`, `match`, `case`, `default`,
`end`, `break`, `continue`, `print`, `pause`, `on`) or it assigns
(`-- @x = …`, `-- @x++`); `-- @author someone` stays a comment.

## 2. Values

| Kind | Written | Notes |
|---|---|---|
| number | `123`, `-4`, `1.5` | integers stay integers |
| string | `'eu'`, `"eu"` | both quotes are strings here (in SQL `"` is a name; inside a directive it isn't). `\'` / `''` inside |
| template | `` `shard ${i}: ${n} rows` `` | `${…}` takes any expression |
| boolean | `true`, `false` | |
| null | `null` | |
| date | `202611 \| date yyyymm`, `'2026-11-01' \| date` | see §4; a date keeps the precision it was read with |
| list | `1, 2, 3` · `[1, 2, 3]` · `(1, 2, 3)` · `{1, 2, 3}` | all four the same; elements of any kind |
| dictionary | `{"key1": 1, "key2": 2}` · `{key1: 1}` | `{…}` with `:` inside is a dictionary; `{}` is an empty one |
| query | `(select …)` / `(with … select …)` | runs on the tab's connection, see below |

### Values from a query

A query in parentheses runs when the directive is reached, in the same
session as the script (so it sees the script's uncommitted changes):

| The query returns | The value is |
|---|---|
| one row, one column | that value (`@set n = (select count() from t)`) |
| one column, any rows | a list of values |
| several columns | a list of rows; a row is a dictionary by column name (`t.name`, `t["name"]`) |
| no rows | `null` for the first case, an empty list otherwise |

`| list` makes a one-row, one-column answer a list anyway.

Column types come through: numbers as numbers, dates as dates, the rest as
strings, NULL as `null`.

## 3. Variables

```sql
-- @set x = 123
-- @var x = 123          -- @var is the same as @set
-- @x = x - 1            -- assignment to an existing variable
-- @x += 2               -- also -=, *=, /=
-- @x++                  -- and @x--
```

Names are letters, digits and `_`, not starting with a digit, compared in
any case (`X` is `x`), as SQL names are.

Scope:

- A variable lives from where it's set to the end of the script; in a
  notebook, for the cells below too (in the order they run).
- A loop variable (`@for i in …`) lives inside its loop only.
- A variable used in SQL but never set is asked for before the run, in the
  same window that asks for `:name` / `&name` now (remembered values and
  all). A variable set by a directive is never asked.

### Elements and expressions

```
ls[0]          first element (negative counts from the end: ls[-1])
dk["key1"]     dk.key1          a dictionary's value
t.name         a row's column
len(ls)        a list's / dictionary's / string's length
upper(s) lower(s) keys(dk) values(dk)
```

Operators, loosest first:

```
or
and
not
=  ==  !=  <>  <  <=  >  >=  in  (x in ls, "key1" in dk)
+  -           (+ on strings joins them; a list + a list joins the lists)
*  /  %
-x             unary minus
|              pipe (conversion), §4
a.b  a[i]  f(…)  (…)
```

Comparisons between a number and a string compare numbers when the string
reads as one; dates compare as dates.

## 4. Conversions: `|`

```
x | number                 '12' → 12
x | string                 12 → '12'
x | date                   '2026-11-01' (ISO, with or without time) → a date
x | date yyyymm            202611 / '202611' read with the format → a date
x | list                   a single value → a list of one
```

The same pipe formats a value where it's used (§5): a date piped to
`date yyyymmdd` while it's already a date becomes text in that format.

Date format tokens (Oracle style, any case): `yyyy`, `yy`, `mm`, `dd`,
`hh24`, `hh`, `mi`, `ss`, `q` (quarter), `ddd` (day of year); anything else
is taken as it is. Unquoted, a format is its words glued by `-` `.` `/`
`:` `_` (`yyyy-mm-dd`, `dd.mm.yyyy`, `yyyymmdd_hh24`); with a space it's
quoted (`| date 'dd.mm.yyyy hh24:mi'`).

A date remembers its precision: `202611 | date yyyymm` is the month
2026-11; used without a format it prints as `2026-11-01`.

## 5. Variables in SQL: `:` and `&`

As now: **`:` puts in a value, `&` puts in text as it is.**

| Value | `:x` | `&x` |
|---|---|---|
| number `7` | `7` | `7` |
| string `eu` | `'eu'` (quotes inside doubled) | `eu` |
| date | `'2026-11-01'`; Oracle: `DATE '2026-11-01'` (with time: `TIMESTAMP '…'`) | `2026-11-01` |
| list `1, 2, 3` | `1, 2, 3` — for `in (:ls)` | `1,2,3` |
| list `'a', 'b'` | `'a', 'b'` | `a,b` |
| `null` | `NULL` | empty |
| dictionary | an error (take a field) | an error |

```sql
from events_&i                     -- events_7
from events_&i._old                -- the dot ends the name: events_7_old
from &t.database..&t.name          -- a row's fields; the first dot after a name ends it, so `..` = one dot
where day = :d and region in (:regions)
from events_&{d | date yyyymmdd}   -- {…} takes an expression (a pipe, an element, arithmetic)
where n > :{limit * 2}
```

`::type` casts, `:=`, ClickHouse's `{p:Type}` and names with no value stay
as they are. `&` goes in strings and quoted names too, as SQL*Plus has it
(`like 'events_&i%'`), and right after a name (`events_&i`); `:` doesn't
go in strings. Neither goes in comments.

## 6. Loops

```sql
-- @for i in 0 .. 15               -- a range, both ends in
-- @for i in 0 .. 100 step 10
-- @for i in 15 .. 0               -- downwards (step -1 by itself)
-- @for i in 01 .. 16              -- leading zeros keep the width: 01, 02 … 16
-- @for c in 'a' .. 'f'            -- letters
-- @for d in 202611 .. 202612 | date yyyymm step day
-- @for d in '2026-11-01 00:00' .. '2026-11-01 23:00' | date step hour
-- @for x in [1, 2, 3]             -- a list (any of the four forms)
-- @for x in ls                    -- a list from a variable
-- @for t in (select database, name from system.tables where name like 'ev_%')
-- @for n, x in ls                 -- with the pass number (from 0)
-- @for k, v in dk                 -- a dictionary: key and value
…
-- @end for
```

- **Date ranges.** `step` takes `minute`, `hour`, `day`, `week`, `month`,
  `quarter`, `year`, with a count before it if wanted (`step 2 hour`). A
  pipe after a range reads both ends (`202611 .. 202612 | date yyyymm`); the
  long form, each end piped, works too. **The end is in up to the end of
  its precision**: `202611 .. 202612 | date yyyymm step day` is every day of
  November and December. Without `step`, a date range steps by its
  precision (months here).
- **The list is worked out once**, when the loop starts; changing the
  variable inside doesn't change the passes.
- `@break` leaves the loop, `@continue` goes to the next pass.

### While

```sql
-- @while x < 10
-- @x++
-- @end while

-- @while (select count() from system.mutations where not is_done) > 0
-- @pause 10s
-- @end while
```

The condition is checked before every pass. A query in it runs each time.

`@end` alone closes whichever block is open; `@end for` / `@end while` /
`@end if` / `@end match` are checked against what they close.

**Runaway guard**: Stop ends any loop at once (the running statement is
cancelled too). A loop that makes 100 000 passes without running any SQL
stops with an error, as it surely spins.

## 7. Conditions

```sql
-- @if n = 0
-- @print('nothing to do')
-- @elif n < 1000
insert into small select * from src;
-- @else
insert into big select * from src;
-- @end if

-- @match region                  -- the first case that fits; none fits: @default
-- @case 'eu', 'uk'               -- any of these values
optimize table events_eu final;
-- @case 1 .. 5                   -- a range (dates too)
-- @case [6, 7, 8]                -- in a list
-- @default
-- @print('unknown region ', region)
-- @end match
```

`@else if` is the same as `@elif`. A condition is true when it's `true`, a
non-zero number, a non-empty string / list / dictionary; `null` is false.

## 8. Output and the rest

```sql
-- @print('shard ', i, ': done')            -- the arguments joined
-- @print(`shard ${i}: ${n} rows`)
-- @print(t.name, ' → ', d | date yyyymmdd)
-- @print(ls)                                -- [1, 2, 3]; dictionaries {"key1": 1}
-- @pause 10s                                -- also 500ms, 2m
-- @on error continue                        -- or stop (the default)
```

- `@print` writes to Output (where DBMS_OUTPUT and RAISE NOTICE go).
- `@on error continue`: a statement failing in a loop is logged and the loop
  goes on with the next pass; `stop` ends the run there. It holds from where
  it's written to the end of the script (or the next `@on error`).

## 9. Running it

- **Read through first.** The whole script's directives are parsed before
  anything runs; a mistake (an unclosed loop, `@end while` closing a `@for`,
  an unknown name in an expression) is shown with its line and nothing runs.
- **(Not yet.) Before a run with loops**, the run bar says how much is ahead when it
  can be known without the server: "16 passes × 2 statements". **Dry run**
  ("Show SQL") expands the script with the values it would get and opens
  the result in a new tab without running anything; queries in directives
  are run for it (read only), or left as `(query)` when the dry run is told
  not to touch the server.
- **While it runs**: progress "7/16 · i = 06" by the results' tabs; each
  pass's statements log as usual (time, rows); results of the last pass's
  SELECTs are shown (each pass's result takes the place of the one before,
  to keep memory flat; `@print` is the way to keep a figure from each
  pass).
- **The session**: everything runs in the connection's session, one
  statement after another, like a script now — transactions and auto-commit
  as set; the "uncommitted changes" prompts work as before.
- **Stop** cancels the statement running and ends the run; a statement
  queued behind it doesn't start.

## 10. Grammar

```
directive  = "@" ( set | assign | for | while | if | elif | "else" | match
                 | case | "default" | end | "break" | "continue"
                 | print | pause | onerror ) [ "--" comment ]
if         = "if" expr
elif       = ("elif" | "else" "if") expr
match      = "match" expr
case       = "case" ( range | expr { "," expr } )
set        = ("set" | "var") name "=" expr
assign     = name ( ("=" | "+=" | "-=" | "*=" | "/=") expr | "++" | "--" )
for        = "for" name [ "," name ] "in" source
source     = range | expr
range      = expr ".." expr [ pipe ] [ "step" [ number ] [ unit ] ]
unit       = "minute" | "hour" | "day" | "week" | "month" | "quarter" | "year"
while      = "while" expr
end        = "end" [ "for" | "while" | "if" | "match" ]
print      = "print" "(" [ expr { "," expr } ] ")"
pause      = "pause" duration                 (10s, 500ms, 2m)
onerror    = "on" "error" ( "stop" | "continue" )

expr       = or
or         = and { "or" and }
and        = not { "and" not }
not        = [ "not" ] cmp
cmp        = add [ ( "=" | "==" | "!=" | "<>" | "<" | "<=" | ">" | ">=" | "in" ) add ]
add        = mul { ( "+" | "-" ) mul }
mul        = unary { ( "*" | "/" | "%" ) unary }
unary      = [ "-" ] piped
piped      = post { "|" conv }
conv       = "number" | "string" | "list" | "date" [ format ]
post       = atom { "." name | "[" expr "]" | "(" args ")" }
atom       = number | string | template | "true" | "false" | "null"
           | name | list | dict | "(" query ")" | "(" expr ")" | "(" expr "," … ")"
list       = expr "," expr { "," expr }        (bare, in a @set / @for only)
           | "[" … "]" | "{" … "}" | "(" … ")"
dict       = "{" key ":" expr { "," key ":" expr } "}"
```

In SQL text:

```
:name  :name.field  :{expr}       a value
&name  &name.field  &{expr}       text as it is
&name.                            the dot ends the name
```

## 11. Open questions

- Results of every pass gathered into one table (a UNION of the passes'
  SELECTs) — useful for "count per shard"; `@print` covers it for now.
- Running passes in parallel (several sessions) — ClickHouse would like it;
  later, with a limit on sessions.
- Hints and hover in the editor: directive words, variables in scope, the
  value of `&i` on hover during a run.
