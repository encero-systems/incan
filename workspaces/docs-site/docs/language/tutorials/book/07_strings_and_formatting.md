# 7. Strings and formatting

<div class="inc-book-progress" aria-label="Chapter 7 of 13"><div class="inc-book-progress__meta"><strong>Chapter 7 of 13</strong><span>Strings and formatting</span></div><div class="inc-book-progress__bar" aria-hidden="true"><span style="--inc-progress: 53.8%"></span></div></div>

Strings are `str`, and you’ll often build output using f-strings.

## String methods

```incan
def main() -> None:
    raw = "  Alice  "
    cleaned = raw.strip().lower()
    println(cleaned)
```

## F-strings (interpolation)

```incan
def main() -> None:
    name = "Alice"
    age = 30
    println(f"{name} age={age}")
```

## Try it

1. Normalize an input string with `strip().lower()`.
2. Build an output line using an f-string.
3. Use one string method you didn’t use yet (for example `upper()`).

??? example "One possible solution"

    ```incan
    def main() -> None:
        raw = "  Alice  "
        cleaned = raw.strip().lower()
        println(f"cleaned={cleaned}")
        println(cleaned.upper())
    ```

## Where to learn more

- Full strings guide: [Strings](../../reference/strings.md)

<nav class="inc-prev-next" aria-label="Book chapter navigation"><a href="../06_errors/"><small>Previous chapter</small><strong>← 6. Errors</strong></a><a href="../08_collections_and_iteration/"><small>Next chapter</small><strong>8. Collections and iteration →</strong></a></nav>


