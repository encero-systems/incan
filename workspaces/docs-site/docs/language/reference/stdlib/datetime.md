# std.datetime reference

`std.datetime` provides temporal value types for runtime timing, civil dates and times, fixed UTC offsets, and interval arithmetic.

For a guided walkthrough, see [Dates and times](../../tutorials/dates_and_times.md). For recipes, see
[Dates and times](../../how-to/dates_and_times.md). For the mental model behind the runtime/civil split, see
[Date and time model](../../explanation/datetime_model.md).

The runtime timing layer is backed by Rust's `std::time` through normal Incan Rust interop. The civil calendar layer is source-defined Incan, including normalization, comparison, arithmetic, fixed-offset ISO parsing/formatting, and Python-shaped `strftime` / `strptime` helpers with nanosecond `%f` precision.

## Importing

```incan
from std.datetime import Date, DateTime, DateTimeOffset, Duration, FixedOffset, Instant, SystemTime
```

The implementation is split into `std.datetime.runtime`, `std.datetime.civil`, and `std.datetime.error`; `std.datetime` re-exports the public prelude.

## Runtime timing values

`Duration` is the elapsed-time value type. It wraps Rust `std::time::Duration`, is nonnegative, and has unit factories:

```incan
from std.datetime import Duration

short = Duration.milliseconds(250)
longer = short + Duration.seconds(2)
println(longer.whole_seconds())
```

`Instant` represents a monotonic clock reading:

```incan
from std.datetime import Duration, Instant

start = Instant.now()
stop = start.checked_add(Duration.seconds(2))?
elapsed = stop.duration_since(start)
```

`SystemTime` represents a host system-clock timestamp. Construction from Unix time can fail when the platform cannot represent the requested timestamp, so Unix factories return `Result`:

```incan
from std.datetime import Duration, SystemTime

timestamp = SystemTime.from_unix_seconds(1_700_000_000)?
next = timestamp.checked_add(Duration.seconds(30))?
println(SystemTime.now().unix_seconds() > 0)
```

## Civil values

`Date`, `Time`, and `DateTime` represent calendar dates, wall-clock times, and naive datetimes. `Date.utc_today()` and `DateTime.utc_now()` read the host clock through `SystemTime` and convert the Unix timestamp to UTC civil fields in Incan:

```incan
from std.datetime import Date, DateTime, Time

date = Date.fromisoformat("2026-04-14")?
time = Time.fromisoformat("12:34:56.123456789")?
stamp = DateTime.combine(date, time)
println(stamp.isoformat())
println(Date.utc_today().isoformat())
println(DateTime.utc_now().isoformat())
```

`Date`, `Time`, and `DateTime` support `isoformat()`, `fromisoformat(...)`, `strftime(...)`, and `strptime(...)`. The format surface is Incan-defined and Python-shaped; `%f` formats and parses nanoseconds as 9 fractional digits rather than Python's microsecond ceiling.

```incan
parsed = DateTime.strptime("2026-04-14 07:08:09.123456789", "%F %T.%f")?
println(parsed.strftime("%a %b %_d %H:%M:%S.%f %Y")?)
```

`Date` also supports `weekday()`, `iso_week()`, `day_of_year()`, `quarter()`, and ISO calendar construction with `fromisocalendar(...)`.

## Fixed offsets

`FixedOffset` stores a concrete UTC offset in whole minutes. `DateTimeOffset` pairs a naive `DateTime` with that offset and supports ISO text, `%z`, and `%:z`:

```incan
from std.datetime import DateTime, DateTimeOffset, FixedOffset

stamp = DateTime.fromisoformat("2026-04-14T12:34:56.123456789")?
offset = FixedOffset.hours(1)?
aware = DateTimeOffset(datetime=stamp, offset=offset)

println(aware.isoformat())                    # 2026-04-14T12:34:56.123456789+01:00
println(aware.strftime("%F %T.%f%z")?)        # 2026-04-14 12:34:56.123456789+0100
println(aware.strftime("%F %T.%f%:z")?)       # 2026-04-14 12:34:56.123456789+01:00
```

Named timezone lookup is not part of `std.datetime`. A named zone such as `Europe/Amsterdam` is not one permanent offset; it resolves to an offset for a specific instant or local civil time because daylight-saving and historical rules change. Timezone-aware `today` / `now` helpers and named-zone rule data belong in separately versioned packages such as `pub.timezones`.

## Intervals

`TimeDelta` is a day/time interval. `YearMonthInterval` is a year/month interval. `DateTimeInterval` is a compound interval that normalizes within compatible buckets but does not collapse months into days or years into fixed-length durations.

```incan
from std.datetime import Date, DateTimeInterval, TimeDelta, YearMonthInterval

anchor = Date.fromisoformat("2026-04-14")?
next_week = anchor + TimeDelta.days(7)
quarter_end = anchor + YearMonthInterval.months(3)

normalized = DateTimeInterval.new(months=15, days=1, hours=24)
assert normalized == DateTimeInterval.new(years=1, months=3, days=2)
```

When a `DateTimeInterval` is applied to a civil value, the year/month portion is applied first, then the day/time/fractional portion.

## See also

- [Dates and times tutorial](../../tutorials/dates_and_times.md)
- [Dates and times how-to](../../how-to/dates_and_times.md)
- [Date and time model](../../explanation/datetime_model.md)
- [RFC 058: std.datetime](../../../RFCs/closed/implemented/058_std_datetime.md)
