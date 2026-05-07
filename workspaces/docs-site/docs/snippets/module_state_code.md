```incan title="counters.incn"
pub static hits: int = 0

pub def record_hit() -> None:
    hits += 1
```

```incan title="main.incn"
from counters import hits, record_hit

def main() -> None:
    record_hit()
    record_hit()
    println(hits)
```
