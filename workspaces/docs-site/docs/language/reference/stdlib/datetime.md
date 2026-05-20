# std.datetime reference

`std.datetime` provides temporal value types for runtime timing, civil dates and times, fixed UTC offsets, and interval arithmetic.

For a guided walkthrough, see [Dates and times](../../tutorials/dates_and_times.md). For recipes, see [Dates and times](../../how-to/dates_and_times.md). For the mental model behind the runtime/civil split, see [Date and time model](../../explanation/datetime_model.md).

The runtime timing layer provides elapsed-time values, monotonic clock readings, and host system-clock timestamps. The civil calendar layer is source-defined Incan, including normalization, comparison, arithmetic, fixed-offset ISO parsing/formatting, and Python-shaped `strftime` / `strptime` helpers with nanosecond `%f` precision.

## Importing

```incan
from std.datetime import Date, DateTime, DateTimeOffset, Duration, FixedOffset, Instant, SystemTime
```

The implementation is split into `std.datetime.runtime`, `std.datetime.civil`, and `std.datetime.error`; `std.datetime` re-exports the public prelude.

## Runtime timing values

`Duration` is the elapsed-time value type. It wraps Rust `std::time::Duration`, is nonnegative, and has unit factories such as seconds, milliseconds, microseconds, and nanoseconds.

`Instant` represents a monotonic clock reading for elapsed-time measurement. `SystemTime` represents a host system-clock timestamp. Construction from Unix time can fail when the platform cannot represent the requested timestamp, so Unix factories return `Result`.

## Civil values

`Date`, `Time`, and `DateTime` represent calendar dates, wall-clock times, and naive datetimes. `Date.utc_today()` and `DateTime.utc_now()` read the host clock through `SystemTime` and convert the Unix timestamp to UTC civil fields in Incan.

`Date`, `Time`, and `DateTime` support `isoformat()`, `fromisoformat(...)`, `strftime(...)`, and `strptime(...)`. The format surface is Incan-defined and Python-shaped; `%f` formats and parses nanoseconds as 9 fractional digits rather than Python's microsecond ceiling.

`Date` also supports `weekday()`, `iso_week()`, `day_of_year()`, `quarter()`, and ISO calendar construction with `fromisocalendar(...)`.

## Format directives

`strftime(...)` and `strptime(...)` format strings are ordinary text plus percent directives. In `%Y-%m-%dT%H:%M:%S.%f%z`, `%Y`, `%m`, `%d`, `%H`, `%M`, `%S`, `%f`, and `%z` are directives; `-`, `T`, `:`, and `.` are literal characters that must appear exactly when parsing.

Date, time, and offset support depends on the receiver: `Date` accepts date directives only; `Time` accepts time directives only; `DateTime` accepts date and time directives; `DateTimeOffset` accepts date, time, and fixed-offset directives. Named timezone directive `%Z` is rejected by `std.datetime`.

### Date directives

| Directive | Meaning | Notes |
| --------- | ------- | ----- |
| `%Y` | Year with century | Four digits, such as `2026` |
| `%y` | Year without century | `70`-`99` parse as 1970-1999; `00`-`69` parse as 2000-2069 |
| `%m` | Month number | `01`-`12` |
| `%d` | Day of month | `01`-`31` |
| `%e` | Day of month with space padding | Formats ` 1` by default; parsing accepts an optional leading space |
| `%j` | Day of year | `001`-`366` |
| `%a` | Abbreviated weekday name | `Mon`, `Tue`, ... |
| `%A` | Full weekday name | `Monday`, `Tuesday`, ... |
| `%b` | Abbreviated month name | `Jan`, `Feb`, ... |
| `%h` | Abbreviated month name | Alias for `%b` |
| `%B` | Full month name | `January`, `February`, ... |
| `%w` | Weekday number | Sunday is `0`, Monday is `1`, Saturday is `6` |
| `%u` | ISO weekday number | Monday is `1`, Sunday is `7` |
| `%U` | Week number, Sunday first | `00`-`53`; parsing requires a weekday directive |
| `%W` | Week number, Monday first | `00`-`53`; parsing requires a weekday directive |
| `%V` | ISO week number | `01`-`53`; parsing requires ISO week-year context |
| `%G` | ISO week-numbering year | Four digits |
| `%g` | ISO week-numbering year without century | Two digits |

### Time directives

| Directive | Meaning | Notes |
| --------- | ------- | ----- |
| `%H` | Hour, 24-hour clock | `00`-`23` |
| `%I` | Hour, 12-hour clock | `01`-`12`; use with `%p` or `%P` |
| `%M` | Minute | `00`-`59` |
| `%S` | Second | `00`-`59` |
| `%f` | Fractional second | Formats exactly 9 nanosecond digits; parsing accepts 1-9 digits and pads to nanoseconds |
| `%p` | Uppercase meridiem | `AM` or `PM` |
| `%P` | Lowercase meridiem | `am` or `pm`; parsing accepts either case |

### Offset and literal directives

| Directive | Meaning | Notes |
| --------- | ------- | ----- |
| `%z` | Fixed UTC offset | `DateTimeOffset` only; formats `+HHMM` or `-HHMM`; parsing accepts `Z`, `+HHMM`, `-HHMM`, `+HH:MM`, and `-HH:MM` |
| `%:z` | Fixed UTC offset with colon | `DateTimeOffset` only; formats `+HH:MM` or `-HH:MM`; parsing accepts the same fixed-offset inputs as `%z` |
| `%Z` | Named timezone | Not supported by `std.datetime` |
| `%%` | Literal percent sign | Formats and parses `%` |
| `%n` | Newline | Formats and parses `\n` |
| `%t` | Tab | Formats and parses `\t` |

### Compound aliases

| Alias | Expands to |
| ----- | ---------- |
| `%F` | `%Y-%m-%d` |
| `%D` | `%m/%d/%y` |
| `%x` | `%m/%d/%y` |
| `%R` | `%H:%M` |
| `%T` | `%H:%M:%S` |
| `%X` | `%H:%M:%S` |
| `%r` | `%I:%M:%S %p` |
| `%c` | `%a %b %_d %H:%M:%S %Y` |

Numeric formatting directives accept padding modifiers: `%-d` disables padding, `%_d` uses spaces, and `%0d` uses zeroes. Modifiers are not supported on compound aliases such as `%F`, `%T`, or `%c`; `%:z` uses `:` for the offset separator rather than numeric padding.

## Fixed offsets

`FixedOffset` stores a concrete UTC offset in whole minutes. `DateTimeOffset` pairs a naive `DateTime` with that offset and supports ISO text, `%z`, and `%:z`.

Named timezone lookup is not part of `std.datetime`. A named zone such as `Europe/Amsterdam` is not one permanent offset; it resolves to an offset for a specific instant or local civil time because daylight-saving and historical rules change. Timezone-aware `today` / `now` helpers and named-zone rule data belong in separately versioned packages such as `pub.timezones`.

## Intervals

`TimeDelta` is a day/time interval. `YearMonthInterval` is a year/month interval. `DateTimeInterval` is a compound interval that normalizes within compatible buckets but does not collapse months into days or years into fixed-length durations.

When a `DateTimeInterval` is applied to a civil value, the year/month portion is applied first, then the day/time/fractional portion.

## See also

- [Dates and times tutorial](../../tutorials/dates_and_times.md)
- [Dates and times how-to](../../how-to/dates_and_times.md)
- [Date and time model](../../explanation/datetime_model.md)
- [RFC 058: std.datetime](../../../RFCs/closed/implemented/058_std_datetime.md)
