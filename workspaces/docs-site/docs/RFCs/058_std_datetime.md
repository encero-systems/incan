# RFC 058: `std.datetime` — temporal values, intervals, and runtime timing

- **Status:** Planned
- **Created:** 2026-04-14
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 022 (namespaced stdlib modules and compiler handoff)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 055 (`std.fs` path-centric filesystem APIs)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/292
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC proposes `std.datetime` as Incan's standard library home for temporal values. The module covers runtime timing (`Duration`, `Instant`, `SystemTime`), civil and analytics-facing temporal values (`Date`, `Time`, `DateTime`, `DateTimeTZ`), and first-class interval types (`TimeDelta`, `YearMonthInterval`, `DateTimeInterval`). The design intentionally combines a Rust-shaped runtime timing model with a more analytics- and Substrait-shaped civil time model so Incan has one coherent temporal vocabulary for scheduling, timestamps, temporal arithmetic, and data-facing date or time work without dropping into Rust or treating time as raw strings and integers.

## Motivation

Time is a foundational hole in the current stdlib surface. Programs need to measure elapsed work, represent timestamps, parse and format calendar values, and perform date arithmetic in analytics pipelines. Today, that story is fragmented: elapsed-time measurement pushes users toward Rust interop, while calendar-style dates, datetimes, and intervals have no clear standard home at all.

This matters across a broad slice of Incan code:

- CLIs, services, and workflow systems need durations, deadlines, and retry intervals.
- Analytics and ETL work need dates, datetimes, and interval arithmetic that are honest about month- and year-based semantics.
- Filesystem and process APIs eventually need a shared temporal vocabulary for metadata, timeouts, and scheduling.
- Tests need stable construction, parsing, and comparison helpers.

`std.datetime` should therefore give Incan one coherent temporal vocabulary instead of a mix of strings, integers, and Rust escape hatches.

## Goals

- Provide a standard runtime timing story with `Duration`, `Instant`, and `SystemTime`.
- Provide a standard civil and analytics-facing temporal story with `Date`, `Time`, `DateTime`, and `DateTimeTZ`.
- Provide first-class interval types for analytics and temporal arithmetic rather than flattening everything into one duration-shaped abstraction.
- Support arithmetic, comparison, parsing, and formatting for the committed temporal types.
- Keep the user-facing surface Incan-first even when the runtime maps onto Rust primitives underneath.
- Keep core timezone support stable by standardizing `UTC` and fixed offsets in the stdlib while allowing richer named-zone support to live in separately versioned packages.

## Non-Goals

- Defining cron-like recurrence rules or workflow scheduler DSLs in this RFC.
- Standardizing named IANA timezone-database support in the core standard library.
- Providing locale-sensitive formatting as the primary formatting model.
- Replacing domain-specific time libraries that may later exist for finance, calendars, or recurrence.

## Guide-level explanation

Authors should be able to use `std.datetime` for both elapsed-time measurement and data-facing temporal work.

```incan
from std.datetime import Duration, Instant

start = Instant.now()
run_job()?
elapsed = start.elapsed()

if elapsed > Duration.seconds(5):
    println("job was slow")
```

```incan
from std.datetime import DateTimeTZ, FixedOffset

created_at = DateTimeTZ.now(tz=FixedOffset.hours(1))?
parsed = DateTimeTZ.fromisoformat("2026-04-14T12:34:56+01:00")?

if parsed < created_at:
    println(parsed.isoformat())
```

```incan
from std.datetime import Date, TimeDelta, YearMonthInterval

anchor = Date.strptime("2026-04-14", "%Y-%m-%d")?
next_week = anchor + TimeDelta.days(7)
quarter_end = anchor + YearMonthInterval.months(3)
```

The mental model should be simple:

- use `Duration`, `Instant`, and `SystemTime` for runtime timing and deadlines;
- use `Date`, `Time`, `DateTime`, and `DateTimeTZ` for civil and external timestamp work;
- use interval types for analytics-facing temporal arithmetic.

## Reference-level explanation

### Module scope

`std.datetime` is the standard library home for Incan temporal values. The committed family includes:

- `Duration`
- `Instant`
- `SystemTime`
- `Date`
- `Time`
- `DateTime`
- `DateTimeTZ`
- `UTC`
- `FixedOffset`
- `TimeDelta`
- `YearMonthInterval`
- `DateTimeInterval`

There is no separate `std.time` module in this design. Runtime timing and calendar-facing time belong in one coherent namespace because users need them together.

### Semantic families

The module distinguishes three semantic families:

- runtime timing values for elapsed measurement and machine-clock work: `Duration`, `Instant`, `SystemTime`
- civil and external-facing values for dates, times, and timestamps: `Date`, `Time`, `DateTime`, `DateTimeTZ`
- analytics-facing interval values for temporal arithmetic: `TimeDelta`, `YearMonthInterval`, `DateTimeInterval`

That split is normative. The public contract must not blur fixed elapsed time, civil timestamps, and calendar-relative intervals into one mushy abstraction.

### Timestamp model

The timestamp split is explicit:

- `DateTime` is a naive datetime with no timezone or offset attached.
- `DateTimeTZ` is a timezone-aware datetime.

Core stdlib timezone support is intentionally narrow:

- `UTC` is part of the core surface.
- `FixedOffset` is part of the core surface.
- Named IANA timezone support is not part of `std.datetime`.

That richer timezone story is expected to live in separately versioned packages, for example:

```incan
from pub::timezones @ 0.1 import TimeZone
```

### Interval model

The interval model is also explicit:

- `Duration` is the runtime elapsed-time type.
- `TimeDelta` is the Python-friendly day/time-style interval.
- `YearMonthInterval` is the month/year-style interval.
- `DateTimeInterval` is the compound interval type for analytics-facing temporal arithmetic.

This split is intentional. `Duration` is not the main analytics interval abstraction. Month- and year-based arithmetic must not be smuggled through fake fixed-length durations.

### Arithmetic boundary

The arithmetic boundary is normative:

- `Instant` composes with `Duration`.
- `SystemTime` composes with `Duration`.
- `Date`, `DateTime`, and `DateTimeTZ` compose with `TimeDelta`, `YearMonthInterval`, and `DateTimeInterval`.
- `Instant` must not compose with `TimeDelta`, `YearMonthInterval`, or `DateTimeInterval`.

That keeps runtime timing and analytics-facing interval arithmetic distinct.

### Parsing and formatting

The parsing and formatting story is complete and Python-shaped:

- canonical ISO helpers such as `isoformat()` and `fromisoformat(...)` belong in the contract;
- general `strftime(...)` and `strptime(...)`-style formatting and parsing belong in the contract as well.
- fractional-second parsing and formatting must support up to 9 digits of precision.

However, Incan should standardize the supported format directives itself. The surface should look like Python, but it must not inherit Python's host-libc variability as part of the public contract.

### Constructor and factory surface

The constructor and factory surface should be broad but consistent:

- `Duration` uses unit-based factories such as `weeks(...)`, `days(...)`, `hours(...)`, `minutes(...)`, `seconds(...)`, `milliseconds(...)`, `microseconds(...)`, and `nanoseconds(...)`.
- `Instant` exposes `now()` and `elapsed()` and composes with `Duration`.
- `SystemTime` exposes `now()`.
- `Date`, `Time`, `DateTime`, and `DateTimeTZ` support direct structural construction plus `fromisoformat(...)` and `strptime(...)`.
- `Date` exposes `today()`.
- `DateTime` exposes `now()`.
- `DateTimeTZ` exposes `now(tz=...)`.
- `TimeDelta` uses unit-based factories analogous to `Duration`, including nanosecond precision.
- `YearMonthInterval` uses explicit `years(...)` and `months(...)` factories.
- `DateTimeInterval` uses an explicit composite constructor with keyword-style fields such as `years`, `months`, `days`, `hours`, `minutes`, `seconds`, and fractional-second parts.

Python's constructor and parse/format surface is the right DX reference here, but not its precision ceiling. The north-star precision for `Time`, `DateTime`, `DateTimeTZ`, `Duration`, and `TimeDelta` is nanoseconds rather than microseconds.

### `TimeDelta` and interval naming

The Python-shaped `TimeDelta` name remains the canonical public spelling because it is familiar and clear to users coming from Python. However, the module may also expose `DayTimeInterval` as an alias because that name aligns better with analytics and Substrait-style vocabulary.

That dual naming is justified here because both mental models are important in Incan:

- Python-style application and data users will look for `TimeDelta`;
- analytics-minded users will recognize `DayTimeInterval` immediately.

The public contract should not generalize this into alias proliferation across the entire module. It is a targeted bridge for one type whose semantics are stable and already clearly defined.

### `DateTimeInterval` semantics

`DateTimeInterval` is a single public compound interval type with structured internal components. It is not a scalar duration.

Its semantics should be:

- one public value containing year/month, day/time, and fractional-second components;
- safe normalization within each compatible bucket;
- no collapsing across the calendar-relative versus fixed-time boundary.

Normalization should include examples such as:

- `1500 milliseconds -> 1 second 500 milliseconds`
- `1500 microseconds -> 1 millisecond 500 microseconds`
- `1500 nanoseconds -> 1 microsecond 500 nanoseconds`
- `90 seconds -> 1 minute 30 seconds`
- `25 hours -> 1 day 1 hour`
- `15 months -> 1 year 3 months`

But the module must not normalize:

- `1 month -> 30 days`
- `1 year -> 365 days`

Comparison and equality are also intentionally constrained:

- `DateTimeInterval` must not define a total ordering with `<`, `<=`, `>`, or `>=`;
- normalized structural equality is valid;
- equality is fieldwise after normalization, not "same effect on every possible anchor date."

So examples such as the following should hold:

- `DateTimeInterval(months=15) == DateTimeInterval(years=1, months=3)`
- `DateTimeInterval(days=1, hours=24) == DateTimeInterval(days=2)`
- `DateTimeInterval(months=1) != DateTimeInterval(days=30)`

When a `DateTimeInterval` is applied to `Date`, `DateTime`, or `DateTimeTZ`, the order of application must be fixed and documented:

- year/month portion first
- day/time/fractional portion second

### Core calendar surface

The core contract should include more than raw field access, but it should stop short of becoming a full calendaring framework.

Included in the module contract:

- field accessors such as year, month, day, hour, minute, second, and nanosecond;
- arithmetic and comparison where meaningful;
- parsing and formatting;
- `weekday()`;
- `iso_week()`;
- `day_of_year()`;
- `quarter()`;
- ISO calendar conversion helpers such as `fromisocalendar(...)`-style construction where appropriate.

Explicitly outside the module contract:

- locale-sensitive naming as a core semantic feature;
- non-Gregorian calendar systems;
- holiday or business-calendar logic;
- humanized relative-time phrases such as "3 days ago."

## Design details

### Why `std.datetime` instead of `std.time`

The module is broader than runtime timing. It covers dates, times, naive and aware datetimes, and multiple interval families in addition to `Duration` and `Instant`. Calling that whole surface `std.time` would undersell the civil and analytics half of the design. `std.datetime` is the more honest name.

### Why runtime timing and calendar values live together

Real programs move between both worlds constantly: they read timestamps, compare them, add intervals, and also measure elapsed work or set deadlines. One module keeps the mental model coherent and prevents the temporal surface from fragmenting across multiple partially overlapping namespaces.

### Python-shaped DX, Rust-shaped runtime, analytics-shaped intervals

This RFC deliberately blends three influences:

- Python for public parsing and formatting ergonomics and familiar names such as `TimeDelta`;
- Rust for runtime timing concepts such as `Duration`, `Instant`, and `SystemTime`;
- analytics and Substrait-style thinking for the interval taxonomy and the explicit split between naive and aware timestamp forms.

The goal is not to mirror any one of those ecosystems mechanically. The goal is to produce a better Incan temporal model than any single source provides by itself.

### Why named IANA timezones stay out of core stdlib

Named timezone databases are useful, but they also bring churn, distribution concerns, and data-update cadence questions that should not destabilize the core standard library. The core contract therefore keeps timezone awareness stable and minimal with `UTC` and `FixedOffset`, while richer named-zone support belongs in separately versioned packages. That plays well with Incan's package model and avoids tying timezone-data updates to core language releases.

### Interaction with existing and future features

- `std.fs` may eventually expose timestamps through metadata surfaces that should reuse `std.datetime` types.
- Process or workflow RFCs will likely depend on `Duration` for timeouts and scheduling.
- Analytics and data RFCs should use the interval taxonomy here rather than inventing their own competing time vocabulary.
- Rust interop remains the escape hatch for host-specific or high-precision temporal behavior not standardized here.

### Compatibility / migration

This feature is additive. Existing code using raw integers or strings for time-like data keeps working, but new stdlib APIs should converge on this temporal vocabulary once `std.datetime` exists.

## Alternatives considered

1. **Only runtime timing in stdlib**
   - Too small. It leaves calendar, analytics, and timestamp work fragmented.

2. **Only `datetime`-style calendar values**
   - Too incomplete. It leaves elapsed-time measurement and timeout work without a standard story.

3. **One generic `Interval` type**
   - Too vague. It would blur fixed elapsed time, day/time intervals, and year/month intervals that behave differently in real analytics and scheduling work.

4. **Named timezone support in core stdlib**
   - Viable, but less stable. It couples the core temporal API to timezone-database churn and distribution policy.

5. **Rust interop only**
   - Too implementation-shaped for ordinary Incan code and examples.

## Drawbacks

- Time and interval semantics are notoriously easy to get subtly wrong.
- A broad temporal surface increases the design space substantially compared with narrower utility modules.
- Keeping named timezone support out of core stdlib means some users will need an extra package for richer awareness semantics.

## Layers affected

- **Stdlib / runtime**: must provide the temporal types, interval types, and documented arithmetic and formatting semantics.
- **Language surface**: the module, types, constructors, operators, and methods must be available as specified.
- **Execution handoff**: implementations must preserve arithmetic, comparison, parsing, and formatting behavior without leaking backend quirks.
- **Docs / examples**: should standardize how Incan code measures elapsed time, works with timestamps, and performs interval arithmetic.

## Design Decisions

- The module is `std.datetime`; there is no separate `std.time` namespace.
- `std.datetime` includes runtime timing types, civil timestamp types, and analytics-facing interval types in one module.
- The runtime timing family is `Duration`, `Instant`, and `SystemTime`.
- The civil timestamp family is `Date`, `Time`, `DateTime`, and `DateTimeTZ`.
- `DateTime` is naive; `DateTimeTZ` is aware.
- The interval family is `TimeDelta`, `YearMonthInterval`, and `DateTimeInterval`.
- `Duration` is the runtime elapsed-time type, not the main analytics interval abstraction.
- `Instant` composes with `Duration` only; analytics interval types do not apply to `Instant`.
- Core timezone support is limited to `UTC` and `FixedOffset`.
- Named IANA timezone support is intentionally outside the core standard library and belongs in separately versioned packages.
- Parsing and formatting follow Python's overall `isoformat` / `fromisoformat` and `strftime` / `strptime` model, but the supported directives are standardized by Incan rather than inherited from host libc behavior.
- The constructor and factory surface is broad and explicit: direct constructors for structural values, `now()` / `today()` factories where appropriate, `fromisoformat(...)`, `strptime(...)`, and unit-based interval factories including nanoseconds.
- Nanosecond precision is part of the north-star contract for `Duration`, `TimeDelta`, `Time`, `DateTime`, and `DateTimeTZ`.
- `TimeDelta` remains the canonical public name, with `DayTimeInterval` allowed as an alias.
- `DateTimeInterval` is a single compound public type with structured components, safe normalization within compatible buckets, structural equality after normalization, and no total ordering.
- Applying a `DateTimeInterval` to civil temporal values uses a fixed order: year/month first, then day/time/fractional components.
- The core calendar surface includes `weekday`, `iso_week`, `day_of_year`, `quarter`, and ISO calendar conversion helpers, while excluding locale calendars, business calendars, and humanized relative-time strings.
