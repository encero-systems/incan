# Dates and times

This tutorial builds a small subscription schedule with `std.datetime`. It uses UTC clocks for "now", civil values for dates users type into forms, fixed offsets for external timestamps, and interval types for arithmetic.

## Import the pieces

Most application code can import from `std.datetime` directly:

```incan
from std.datetime import (
    Date,
    DateTime,
    DateTimeError,
    DateTimeOffset,
    FixedOffset,
    Time,
    TimeDelta,
    YearMonthInterval,
)
```

`std.datetime` re-exports the public runtime and civil APIs. Reach for submodules only when you are reading stdlib source or intentionally keeping imports narrow.

## Read UTC from the host clock

Use UTC factories when you need a civil date or datetime anchored to the host system clock:

```incan
from std.datetime import Date, DateTime

def print_clock() -> None:
    println(Date.utc_today().isoformat())
    println(DateTime.utc_now().isoformat())
```

These helpers deliberately return UTC civil values. Named timezones such as `Europe/Amsterdam` are not part of `std.datetime`; they require rule data that belongs in separately versioned packages.

## Parse user input

Civil constructors return `Result` because calendar data can be invalid:

```incan
from std.datetime import Date, DateTimeError, Time

def read_start_date(value: str) -> Result[Date, DateTimeError]:
    return Date.fromisoformat(value)

def read_cutoff_time(value: str) -> Result[Time, DateTimeError]:
    return Time.fromisoformat(value)
```

`fromisoformat` is the best default for machine-readable input. It accepts values such as `"2026-04-14"` and `"12:34:56.123456789"`.

## Add days and months

Use `TimeDelta` for fixed day/time movement and `YearMonthInterval` when the unit is calendar months or years:

```incan
from std.datetime import Date, DateTimeError, TimeDelta, YearMonthInterval

def trial_end(start: str) -> Result[Date, DateTimeError]:
    signup = Date.fromisoformat(start)?
    return Ok(signup + TimeDelta.days(14))

def renewal_date(start: str, months: int) -> Result[Date, DateTimeError]:
    signup = Date.fromisoformat(start)?
    return Ok(signup + YearMonthInterval.months(months))
```

The distinction matters. Seven days is a fixed amount of elapsed civil days. One month is a calendar operation whose day count depends on the month and year.

## Combine date and time

`DateTime` is a naive civil datetime. It has date and time fields but no offset or named timezone:

```incan
from std.datetime import Date, DateTime, DateTimeError, Time

def renewal_cutoff(date_text: str, time_text: str) -> Result[DateTime, DateTimeError]:
    date = Date.fromisoformat(date_text)?
    time = Time.fromisoformat(time_text)?
    return Ok(DateTime.combine(date, time))
```

Use a naive `DateTime` for calendar records where the timezone lives somewhere else in the domain model, or where the value is intentionally local civil time.

## Format for people and protocols

All civil values support `isoformat()`. Use `strftime(...)` when a protocol, log line, or user-facing display needs a specific shape:

```incan
from std.datetime import DateTime, DateTimeError

def render_stamp(stamp: DateTime) -> Result[None, DateTimeError]:
    println(stamp.isoformat())
    println(stamp.strftime("%a %b %_d %H:%M:%S.%f %Y")?)
    return Ok(None)
```

The format surface is Python-shaped. Incan's `%f` is intentionally wider than Python's: it formats and parses nine nanosecond digits. The full directive table is in the [`std.datetime` reference](../reference/stdlib/datetime.md#format-directives).

## Emit a fixed-offset timestamp

Use `DateTimeOffset` when an external system needs a concrete UTC offset in the timestamp:

```incan
from std.datetime import DateTime, DateTimeError, DateTimeOffset, FixedOffset

def stamp_for_amsterdam_winter(local: DateTime) -> Result[str, DateTimeError]:
    offset = FixedOffset.hours(1)?
    stamp = DateTimeOffset(datetime=local, offset=offset)
    return stamp.strftime("%F %T.%f%:z")
```

This stores `+01:00`, not the name `Europe/Amsterdam`. A named timezone package can decide which fixed offset applies to a named zone at a specific instant.

## Put it together

This function parses a signup date, adds one calendar month, combines the result with a cutoff time, and serializes the timestamp with a fixed offset:

```incan
from std.datetime import (
    Date,
    DateTime,
    DateTimeError,
    DateTimeOffset,
    FixedOffset,
    Time,
    YearMonthInterval,
)

def first_invoice_stamp(signup_text: str) -> Result[str, DateTimeError]:
    signup = Date.fromisoformat(signup_text)?
    due_date = signup + YearMonthInterval.months(1)
    cutoff = Time.fromisoformat("17:00:00")?
    local_stamp = DateTime.combine(due_date, cutoff)
    offset_stamp = DateTimeOffset(datetime=local_stamp, offset=FixedOffset.utc())
    return offset_stamp.strftime("%Y-%m-%dT%H:%M:%S.%f%z")
```

The result is explicit about every temporal decision: the signup date is civil, the monthly renewal is calendar-based, the cutoff is a wall-clock time, and the serialized timestamp is fixed-offset UTC.

## See also

- [Dates and times how-to](../how-to/dates_and_times.md)
- [Date and time model](../explanation/datetime_model.md)
- [`std.datetime` reference](../reference/stdlib/datetime.md)
