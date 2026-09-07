# What hashing proves

`std.hash` answers two different questions, and the difference between them is the difference between detecting a mistake and detecting a lie. Choosing the wrong one produces code that looks correct and checks nothing an attacker cares about.

## An unkeyed digest proves consistency, not origin

Every algorithm namespace except `hmac_sha256` is unkeyed. Give it the same bytes and it returns the same digest, for anyone, anywhere. That is exactly what makes it useful for deduplication, cache keys, content addressing, and noticing that a file changed in transit.

It is also exactly what makes it useless for deciding whether to trust a value. The hash formula is public source. Anyone who can read it can invent a value, compute its digest, and hand you both. A digest that matches tells you the pair is internally consistent; it does not tell you who assembled the pair.

That distinction disappears in code, which is why it is worth naming. These two lines look equally careful:

```incan
if sha256.digest(payload) == submitted_digest:
    accept(payload)

if hmac_sha256.verify(key, payload, submitted_tag):
    accept(payload)
```

The first accepts anything a caller is willing to hash. The second accepts only what was tagged with a key the caller does not have.

## A key turns a digest into evidence

`hmac_sha256` mixes a secret into the computation. Reproducing a tag now requires the key as well as the message, so a tag that verifies is evidence the value passed through a process holding that key.

The guarantee is only as good as the key's confinement. If the key crosses the same boundary the value arrives from, an attacker who has the value also has the means to tag it, and the check has become decorative. The useful mental model is that the key marks a trust boundary, and HMAC lets you check whether a value has crossed it.

??? info "Coming from web APIs?"
    This is the mechanism behind signed cookies, webhook signatures, and signed URLs. In each case a service hands out a value it will later be given back, and needs to distinguish the value it issued from one the holder wrote themselves.

## What a MAC does not give you

A MAC is a narrow tool, and three limits matter in practice.

It is **not encryption**. The message travels in the clear; HMAC says nothing about who can read it, only about who could have produced it.

It is **not a signature**. Verifying requires the same secret as tagging, so anyone who can check a tag can also forge one. That makes HMAC unsuitable where a third party must be convinced, or where you need a claim the issuer cannot later deny.

It is **not password hashing**. HMAC is designed to be fast, which is the opposite of what storing a password requires. `std.hash` does not provide password hashing at all.

## Why comparison has to be constant time

`verify` exists as a separate operation rather than leaving callers to compare `digest` output themselves, because the obvious comparison leaks.

An equality check on bytes stops at the first difference. How long it takes therefore depends on how many leading bytes were correct, and an attacker who can time the check can discover a valid tag one byte at a time instead of guessing the whole thing at once. `verify` compares every byte regardless, so its timing carries no information about how close a candidate was.

This is why a keyed comparison is an API rather than an idiom: the correct version is not the one that reads most naturally.

## See also

- [Hashing data](../how-to/hashing_data.md)
- [`std.hash` reference](../reference/stdlib/hash.md)
- [`std.checksum` reference](../reference/stdlib/checksum.md)
