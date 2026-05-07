# Dates and times

This page collects practical recipes for `std.datetime`: reading clocks, parsing user input, formatting output, choosing the right interval type, and working with fixed UTC offsets.

## Choose the right type

| Need | Type |
| ---- | ---- |
| Measure elapsed runtime work | `Instant` plus `Duration` |
| Store a system-clock timestamp | `SystemTime` |
| Store a calendar date | `Date` |
| Store a wall-clock time | `Time` |
| Store a date and time with no offset | `DateTime` |
| Store a date and time with a concrete UTC offset | `DateTimeOffset` |
| Add fixed days, seconds, or nanoseconds | `TimeDelta` |
| Add calendar years or months | `YearMonthInterval` |
| Add a compound civil interval | `DateTimeInterval` |

## Read the clock

Use `Instant.now()` for monotonic measurement. Use `SystemTime.now()` for Unix timestamps. Use `Date.utc_today()` and `DateTime.utc_now()` when application code wants UTC civil values.

```incan
from std.datetime import Date, DateTime, DateTimeError, Duration, Instant, SystemTime

def record_clock() -> Result[None, DateTimeError]:
    started = Instant.now()
    deadline = started.checked_add(Duration.seconds(2))?
    println(deadline.duration_since(started).whole_seconds())

    println(SystemTime.now().unix_seconds())
    println(Date.utc_today().isoformat())
    println(DateTime.utc_now().isoformat())
    return Ok(None)
```

`Instant` is monotonic and should not be serialized as a real-world timestamp. `SystemTime` is the host system clock and can move if the system clock is adjusted.

## Parse ISO input

Use `fromisoformat(...)` for machine-readable input. It returns `Result`, so caller-visible parse failures stay explicit.

```incan
from std.datetime import Date, DateTime, DateTimeError, Time

def parse_inputs(date_text: str, time_text: str) -> Result[DateTime, DateTimeError]:
    date = Date.fromisoformat(date_text)?
    time = Time.fromisoformat(time_text)?
    return Ok(DateTime.combine(date, time))
```

Useful ISO shapes:

| Type | Example |
| ---- | ------- |
| `Date` | `"2026-04-14"` |
| `Time` | `"12:34:56"` |
| `Time` with nanoseconds | `"12:34:56.123456789"` |
| `DateTime` | `"2026-04-14T12:34:56"` |
| `DateTimeOffset` | `"2026-04-14T12:34:56+01:00"` |

## Parse custom text

Use `strptime(value, format)` when input is not ISO-shaped:

```incan
from std.datetime import Date, DateTime, DateTimeError

def parse_report_row(date_text: str, stamp_text: str) -> Result[DateTime, DateTimeError]:
    report_date = Date.strptime(date_text, "%a %b %d %Y")?
    parsed_stamp = DateTime.strptime(stamp_text, "%F %T.%f")?
    println(report_date.isoformat())
    return Ok(parsed_stamp)
```

The directive surface is Python-shaped. `%f` parses fractional seconds as nanoseconds, accepting up to nine digits and normalizing shorter fractions to nanosecond precision.

## Format output

Prefer `isoformat()` for stable machine output. Use `strftime(...)` for display, legacy protocols, or logs.

```incan
from std.datetime import DateTime, DateTimeError

def render(stamp: DateTime) -> Result[None, DateTimeError]:
    println(stamp.isoformat())
    println(stamp.strftime("%Y-%m-%d %H:%M:%S.%f")?)
    println(stamp.strftime("%a %b %_d %H:%M:%S %Y")?)
    return Ok(None)
```

Use `%F` for `%Y-%m-%d`, `%T` for `%H:%M:%S`, and `%f` for nine-digit fractional seconds.

## Add days, months, and compound intervals

Use the interval type that matches the domain:

```incan
from std.datetime import Date, DateTimeError, DateTimeInterval, TimeDelta, YearMonthInterval

def schedule(anchor_text: str) -> Result[None, DateTimeError]:
    anchor = Date.fromisoformat(anchor_text)?

    reminder = anchor + TimeDelta.days(7)
    next_billing_cycle = anchor + YearMonthInterval.months(1)
    compound = anchor + DateTimeInterval.new(months=1, days=3)

    println(reminder.isoformat())
    println(next_billing_cycle.isoformat())
    println(compound.isoformat())
    return Ok(None)
```

Do not replace `YearMonthInterval.months(1)` with a fixed number of days. Month arithmetic depends on the calendar.

## Work with fixed offsets

Use `DateTimeOffset` for timestamps that carry a concrete offset:

```incan
from std.datetime import DateTime, DateTimeError, DateTimeOffset, FixedOffset

def render_offset_timestamp(value: str) -> Result[str, DateTimeError]:
    local = DateTime.fromisoformat(value)?
    stamp = DateTimeOffset(datetime=local, offset=FixedOffset.hours(1)?)
    return stamp.strftime("%F %T.%f%:z")
```

`%z` formats offsets as `+0100`; `%:z` formats offsets as `+01:00`. Parsing accepts `Z` for UTC in ISO fixed-offset input, but named timezone directives are intentionally unsupported in `std.datetime`.

## Handle parse and range failures

Temporal constructors return `Result[..., DateTimeError]` when input can be malformed or out of range:

```incan
from std.datetime import Date, DateTimeError

def print_date(value: str) -> None:
    match Date.fromisoformat(value):
        Ok(date) => println(date.isoformat())
        Err(err) => println(err.message())
```

Use `?` at boundaries that already return `Result`; use `match` when the caller can recover or provide a default.

## See also

- [Dates and times tutorial](../tutorials/dates_and_times.md)
- [Date and time model](../explanation/datetime_model.md)
- [`std.datetime` reference](../reference/stdlib/datetime.md)
- [Error handling](../explanation/error_handling.md)
