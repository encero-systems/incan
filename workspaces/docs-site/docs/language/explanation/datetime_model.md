# Date and time model

`std.datetime` has two layers: runtime timing values and civil calendar values. Keeping those layers separate prevents elapsed-time measurement, host-clock timestamps, and calendar arithmetic from pretending to be the same problem.

## Runtime timing

Runtime timing is for measuring or representing time as the host platform sees it:

- `Duration` is a nonnegative elapsed-time amount.
- `Instant` is a monotonic clock reading for measuring work.
- `SystemTime` is a host system-clock timestamp that can be converted to and from Unix seconds or nanoseconds.

??? info "Coming from Rust?"
    `Duration`, `Instant`, and `SystemTime` mirror Rust's `std::time` vocabulary at the API boundary. Most Incan code only needs the `std.datetime` names; the Rust connection matters when you are reading stdlib source or bridging to Rust libraries.

Use `Instant` for elapsed time:

```incan
from std.datetime import Instant

started = Instant.now()
println(started.elapsed().whole_seconds())
```

Use `SystemTime` for Unix timestamps:

```incan
from std.datetime import SystemTime

stamp = SystemTime.from_unix_seconds(1_700_000_000)?
println(stamp.unix_nanoseconds())
```

## Civil values

Civil values are the dates and times people write down:

- `Date` is a year, month, and day.
- `Time` is an hour, minute, second, and nanosecond.
- `DateTime` is a date and time with no UTC offset.
- `DateTimeOffset` is a `DateTime` plus a concrete `FixedOffset`.

The civil layer is Incan-native. Calendar validation, leap-year rules, ISO parsing, formatting, comparison, and interval arithmetic live in stdlib source rather than in Rust-backed dispatch. The formatting and parsing surface is inspired by the directive vocabulary in `chrono`, `time`, and Python, but the shipped implementation is Incan source-defined.

## Naive and fixed-offset datetimes

`DateTime` is naive. It represents a civil date and time without saying where on Earth that value applies:

```incan
from std.datetime import DateTime

local_meeting = DateTime.fromisoformat("2026-04-14T09:30:00")?
```

That is useful for data that is intentionally local, such as "the store opens at this local wall-clock time", or for records where the timezone is stored separately.

`DateTimeOffset` adds a concrete offset:

```incan
from std.datetime import DateTime, DateTimeOffset, FixedOffset

stamp = DateTimeOffset(
    datetime=DateTime.fromisoformat("2026-04-14T09:30:00")?,
    offset=FixedOffset.hours(2)?,
)
```

This says `+02:00`. It does not say `Europe/Amsterdam`. A named timezone is a rule set, not a single offset, because daylight-saving and historical rules can make the offset depend on the instant being represented.

## Why named timezones are not in stdlib

Named timezone support needs versioned rule data and policy choices for ambiguous or nonexistent local times. Those rules change outside the compiler and stdlib release cycle. `std.datetime` therefore supports fixed `Z` offsets and leaves named-zone lookup to packages such as `pub.timezones`.

The stdlib boundary is:

- parse and format fixed offsets with ISO text, `%z`, and `%:z`
- represent offset-aware timestamps with `DateTimeOffset`
- expose `Date.utc_today()` and `DateTime.utc_now()` as UTC middle-ground factories
- do not ship IANA timezone tables or timezone-aware local `today` / `now`

## Intervals

Incan separates elapsed-time intervals from civil-calendar intervals:

| Type | Meaning |
| ---- | ------- |
| `Duration` | Nonnegative runtime elapsed time |
| `TimeDelta` | Signed civil day/time interval |
| `YearMonthInterval` | Signed calendar year/month interval |
| `DateTimeInterval` | Compound civil interval with year/month and day/time parts |

The distinction is intentional. "One month" cannot be reduced to a fixed number of days without picking a start date. "One second" can be represented as a fixed duration.

```incan
from std.datetime import Date, TimeDelta, YearMonthInterval

anchor = Date.fromisoformat("2026-01-31")?
println((anchor + TimeDelta.days(30)).isoformat())
println((anchor + YearMonthInterval.months(1)).isoformat())
```

Use the interval that matches the product rule. Billing cycles, anniversaries, and month-end workflows usually want `YearMonthInterval`; timeouts and measured work usually want `Duration`; civil reminders often want `TimeDelta`.

## Formatting and parsing

`isoformat()` and `fromisoformat(...)` are the stable default for machine-readable text. `strftime(...)` and `strptime(...)` exist for protocol and display shapes that need directives.

The directive surface is deliberately familiar to Python users, but not identical. Incan uses nanosecond precision: `%f` formats nine digits and parses up to nine fractional second digits. Fixed-offset values support `%z` and `%:z`; named timezone `%Z` is rejected by `std.datetime`.

## See also

- [Dates and times tutorial](../tutorials/dates_and_times.md)
- [Dates and times how-to](../how-to/dates_and_times.md)
- [`std.datetime` reference](../reference/stdlib/datetime.md)
- [RFC 058: std.datetime](../../RFCs/closed/implemented/058_std_datetime.md)
