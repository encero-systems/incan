# Final owner transfers

The original generic reuse expectations remain in this case. The additional controls retain owners across repeated calls, loops, closure captures, conditional consumers, projected reads after a call, and pending outer arguments. Reading a scalar field before a final unconditional consuming call must permit moving the complete owner.

The executable measures allocation and consumption in separate fresh child processes. Each allocates the same model containing 8,388,608 integers. Consuming it after observing its scalar field must stay within 25% plus 16 MiB of the allocation-only peak, excluding another complete list allocation.

The resource probe is reused verbatim from the existing #2426 list-iteration memory contract. RFC 005 excludes unsafe host calls such as `libc::getrusage` from Incan, including inside `unsafe:`; #2426 tracks that removable test bridge. All allocation, consumption, process orchestration and memory assertions are authored in Incan.
