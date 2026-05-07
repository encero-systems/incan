# std.math reference

This page documents the numeric helpers available under `std.math`.
Use this module for standard mathematical constants and functions.

Import with:

```incan
import std.math
```

Access values through the `math` namespace, for example `math.PI` or `math.sqrt(9.0)`.

## Constants

| Name            | Type    |
| --------------- | ------- |
| `math.PI`       | `float` |
| `math.E`        | `float` |
| `math.TAU`      | `float` |
| `math.INFINITY` | `float` |
| `math.NAN`      | `float` |

## Functions

| Function                         | Returns |
| -------------------------------- | ------- |
| `math.gcd(a: int, b: int)`       | `int`   |
| `math.lcm(a: int, b: int)`       | `int`   |
| `math.sqrt(x: float)`            | `float` |
| `math.abs(x: float)`             | `float` |
| `math.floor(x: float)`           | `float` |
| `math.ceil(x: float)`            | `float` |
| `math.round(x: float)`           | `float` |
| `math.pow(x: float, y: float)`   | `float` |
| `math.exp(x: float)`             | `float` |
| `math.log(x: float)`             | `float` |
| `math.log10(x: float)`           | `float` |
| `math.log2(x: float)`            | `float` |
| `math.sin(x: float)`             | `float` |
| `math.cos(x: float)`             | `float` |
| `math.tan(x: float)`             | `float` |
| `math.asin(x: float)`            | `float` |
| `math.acos(x: float)`            | `float` |
| `math.atan(x: float)`            | `float` |
| `math.atan2(y: float, x: float)` | `float` |
| `math.sinh(x: float)`            | `float` |
| `math.cosh(x: float)`            | `float` |
| `math.tanh(x: float)`            | `float` |
| `math.hypot(x: float, y: float)` | `float` |

## Notes

- `math.gcd` returns the greatest common divisor of two integers.
- `math.lcm` returns the lowest common multiple of two integers.
- `math.gcd` and `math.lcm` raise `ValueError` if the mathematical result does not fit Incan's signed 64-bit `int`.
- The floating-point helpers remain thin wrappers over Rust math facilities.
