# CQL reference

CQL is the CXDB query language for filtering contexts through
`GET /v1/contexts/search?q={query}&limit={n}`.

## Examples

```text
tag = "amplifier"
tag = "amplifier" AND user = "alice"
user = "alice"
service ^= "worker"
created > "-24h"
(service = "worker" OR service = "api") AND NOT tag = "test"
```

## Boolean operators

| Operator | Precedence | Example |
| --- | --- | --- |
| `NOT` | Highest | `NOT tag = "test"` |
| `AND` | Medium | `tag = "a" AND user = "b"` |
| `OR` | Lowest | `tag = "a" OR tag = "b"` |

Use parentheses to change the normal precedence.

## Comparison operators

| Operator | Meaning | Example |
| --- | --- | --- |
| `=` | Exact match | `tag = "amplifier"` |
| `!=` | Not equal | `service != "test"` |
| `^=` | Starts with | `tag ^= "amp"` |
| `~=` | Case-insensitive equality | `user ~= "Alice"` |
| `^~=` | Case-insensitive prefix | `service ^~= "API"` |
| `>` | Greater than | `created > "-24h"` |
| `>=` | Greater than or equal | `depth >= 5` |
| `<` | Less than | `created < "2026-01-01"` |
| `<=` | Less than or equal | `depth <= 10` |
| `IN` | List membership | `tag IN ("a", "b")` |

## Fields

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | number | Context ID |
| `tag` | string | Client tag |
| `title` | string | Context title |
| `label` | string | Context label |
| `user` | string | User identity |
| `service` | string | Service name |
| `host` | string | Host name |
| `trace_id` | string | Trace ID |
| `parent` | number | Parent context ID |
| `root` | number | Root context ID |
| `created` | datetime | Creation time |
| `depth` | number | Conversation depth |
| `is_live` | boolean | Active session state |

Relative time values use `-Nh`, `-Nd`, or `-Nm`. Absolute values can use an
ISO 8601 timestamp or a date in `YYYY-MM-DD` form.

## HTTP example

```bash
curl --get 'http://localhost:9010/v1/contexts/search' \
  --data-urlencode 'q=tag = "amplifier" AND created > "-24h"' \
  --data-urlencode 'limit=20'
```

The gateway requires a signed browser session or a personal API token.

## Grammar

```ebnf
query       = expression ;
expression  = or_expr ;
or_expr     = and_expr { "OR" and_expr } ;
and_expr    = unary_expr { "AND" unary_expr } ;
unary_expr  = [ "NOT" ] primary ;
primary     = comparison | "(" expression ")" ;
comparison  = field operator value ;
field       = identifier ;
operator    = "=" | "!=" | "^=" | "~=" | "^~=" | ">" | ">=" | "<" | "<=" | "IN" ;
value       = string | number | date | list ;
list        = "(" value { "," value } ")" ;
```
