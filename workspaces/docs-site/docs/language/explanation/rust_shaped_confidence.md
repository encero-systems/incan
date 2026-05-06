# Rust-shaped confidence

This page is written for Rust users evaluating Incan as an application language. Incan should not be understood as "Rust, but easier"; it is a higher-level authoring surface that keeps Rust-shaped static confidence, Rust ecosystem reach, and explicit failure handling without making ownership syntax dominate ordinary application code.

Maybe you came to Rust after hitting Python's limits: runtime type surprises, packaging friction, performance ceilings, or too much defensive testing around shapes the compiler could have known. Rust probably won you over with explicit errors, traits, predictable builds, memory safety, fearless refactoring, and the feeling that many bugs become compiler diagnostics. But you may still miss how quickly Python lets you sketch application logic, wire together APIs, and keep the domain idea visible on the page.

Incan leans on Rust heavily. It compiles through Rust, maps many core concepts to Rust-backed representations, and treats Rust interop as a real workflow rather than a last-resort escape hatch. But Incan should stand on its own as a language. Its job is not to make you forget Rust exists. Its job is to let you spend more of your attention on the domain model and less of it on repeating borrow shape, lifetime shape, and conversion shape in every everyday API.

## The short version

As a Rust user, you probably do not need to be convinced that compile-time structure is useful. You already know the value of checked types, explicit errors, traits, ownership, and compiler feedback. Incan asks you to make a different shift:

> Incan keeps many Rust-shaped guarantees, but moves more of the routine ownership and lowering decisions into the compiler.

That means Incan code can look direct:

```incan
model User:
    id: int
    name: str

def greeting(user: User) -> str:
    return f"hello {user.name}"

def load_user(id: int) -> Result[User, LoadError]:
    ...

def main() -> None:
    match load_user(42):
        Ok(user) => println(greeting(user))
        Err(err) => println(f"could not load user: {err}")
```

This still has a static shape. `User` is a declared model. `load_user` is fallible, and `main` handles both success and error explicitly. `greeting` does not accept just anything that happens to have a `name` field. The compiler can reason about the program before it runs.

What is missing is also intentional: no `&User`, no lifetime parameter, no `Result<User, E>` ceremony beyond the part that communicates real API meaning, and no user-authored `.clone()` or `.as_ref()` just to satisfy generated Rust in normal code.

## What Rust intuition gets right

Rust's ownership model is valuable because the compiler checks memory-management rules instead of leaving them to runtime convention. The Rust Book describes ownership as rules "that the compiler checks"; if the rules are violated, "the program won't compile".[^rust-ownership] Incan wants that style of early structural pressure, not a dynamic-language free-for-all.

Rust's reference rules are also a good north star. A shared reference lets code read without taking ownership, while a mutable reference makes mutation explicit; the Rust Book says `&mut` in a signature makes it "very clear" that a function mutates the borrowed value.[^rust-borrowing] Incan agrees with the underlying distinction: reading, mutating, storing, returning, and consuming a value are different operations.

Traits transfer well too. Rust uses trait bounds to say a generic type can be "any type that has certain behavior".[^rust-traits] Incan traits play a similar role at the application layer: a trait names a capability, and a function can require that capability without naming one concrete class.

The difference is where Incan asks you to spell things.

## The authoring tradeoff

The tradeoff is simple:

> Incan spends a little more compiler work so you can spend less authoring work.

Compared with hand-written Rust, the compiler has more high-level surface to lower: models, classes, traits, derives, checked defaults, higher-level standard-library APIs, and duckborrowing. That can add compile overhead. In exchange, you write less incidental machinery, and you can get to the domain shape faster.

This should not be framed as "compilation is a problem". Incan is compiled. Compilation is real. The tooling goal is to keep builds warm and fast enough that the trade feels like a small compiler cost for a large authoring win.

The practical question for a Rust user is not "can I express the same thing in Rust?" Often, yes. The practical question is "do I want this layer of the project to be written at Rust's level of ceremony?"

## Duckborrowing

Duckborrowing is the center of the Rust-to-Incan mental shift.

In Rust, ordinary signatures often encode borrow shape directly:

```rust
fn print_user(user: &User) {
    println!("{}", user.name);
}

fn rename_user(user: &mut User, name: String) {
    user.name = name;
}

fn archive_user(user: User) -> ArchivedUser {
    ArchivedUser::from_user(user)
}
```

Those signatures are precise. They tell you whether the function reads, mutates, or consumes the value.

Incan keeps the semantic distinction, but does not make `&`, `&mut`, and lifetimes the normal source-language surface:

```incan
model User:
    id: int
    name: str

class UserEditor:
    user: User

    def print_user(self) -> None:
        println(self.user.name)

    def rename_user(mut self, name: str) -> None:
        self.user.name = name

def archive_user(user: User) -> ArchivedUser:
    return ArchivedUser.from_user(user)
```

The `print_user` method only reads. The `rename_user` method mutates, so the receiver says `mut self`. The `archive_user` function returns a new value from the supplied user. Incan is not pretending those operations are the same. It is letting the source code describe the operation in Incan terms, then letting the compiler choose the generated Rust shape.

The compiler-side duckborrowing planner decides when generated Rust should move, borrow, mutably borrow, clone, call `.into()`, or materialize owned `String` storage. The contributor docs describe it as "the backend ownership-planning layer that lets Incan keep value-oriented source semantics while emitting valid, predictable Rust."[^incan-duckborrowing]

For user code, the rule of thumb is:

- write the direct Incan code first
- use `self` for read-only methods and `mut self` for methods that mutate the object
- rely on ordinary `Result`, `Option`, traits, models, and signatures for the real API contract
- treat manual `.clone()`, `.as_ref()`, `.to_string()`, and `.into()` workarounds in ordinary Incan code as a smell unless you are intentionally shaping a Rust interop boundary

Duckborrowing is not "clone until Rust accepts it". The compiler should preserve moves when it can prove a value is consumed, borrow at Rust interop boundaries where the Rust API expects references, and add `Clone` bounds only when backend-inserted cloning actually requires them.

## What you gain

You gain less signature noise in the layer where Rust's exact borrow spelling is not the point:

```rust
fn summarize<T: Named + Debug>(item: &T) -> Result<String, SummaryError> {
    ...
}
```

becomes:

```incan
def summarize[T with (Named, Debug)](item: T) -> Result[str, SummaryError]:
    ...
```

The Incan version still has a named capability bound, a fallible return type, and a concrete error channel. It just does not ask you to decide at the call-site API level whether `item` should be spelled as `T`, `&T`, `&mut T`, or something with a lifetime parameter. That decision belongs to the compiler unless it is part of the user-facing contract.

A richer example shows the quality-of-life story more clearly:

```incan
type CustomerId = newtype int:
    def from_underlying(value: int) -> Result[CustomerId, str]:
        if value <= 0:
            return Err("customer id must be positive")
        return Ok(CustomerId(value))

type Email = newtype str:
    def from_underlying(value: str) -> Result[Email, str]:
        if "@" not in value:
            return Err("email must contain @")
        return Ok(Email(value.lower()))

trait TaxPolicy:
    def rate_percent(self) -> int

model Customer:
    id: CustomerId
    email: Email
    active: bool = true

model LineItem:
    name: str
    unit_cents: int
    quantity: int

    def subtotal_cents(self) -> int:
        return self.unit_cents * self.quantity

model Order:
    customer: Customer
    items: list[LineItem]

    def subtotal_cents(self) -> int:
        total = 0
        for item in self.items:
            total += item.subtotal_cents()
        return total

@derive(Debug)
model Quote:
    customer_id: CustomerId
    subtotal_cents: int
    tax_cents: int
    total_cents: int

model Vat with TaxPolicy:
    country: str

    def rate_percent(self) -> int:
        if self.country == "NL":
            return 21
        return 0

def quote_order[T with TaxPolicy](order: Order, tax_policy: T) -> Result[Quote, str]:
    if len(order.items) == 0:
        return Err("order must contain at least one line item")

    subtotal = order.subtotal_cents()
    tax = subtotal * tax_policy.rate_percent() // 100
    return Ok(Quote(
        customer_id=order.customer.id,
        subtotal_cents=subtotal,
        tax_cents=tax,
        total_cents=subtotal + tax,
    ))

def main() -> None:
    customer = Customer(
        id=CustomerId(42),
        email=Email("orders@example.com"),
    )
    order = Order(
        customer=customer,
        items=[LineItem(name="Compiler hoodie", unit_cents=6500, quantity=2)],
    )
    match quote_order(order, Vat(country="NL")):
        Ok(quote) => println(f"{quote:?}")
        Err(err) => println(f"could not quote order: {err}")
```

None of these pieces is individually mysterious to a Rust user. The point is how little ownership-facing scaffolding the application layer has to carry. `CustomerId` and `Email` are distinct domain types with checked construction. `TaxPolicy` is a named capability. `Order` is a field-defined shape with a helper method. `Quote` gets debug formatting from a derive instead of handwritten formatting code. `quote_order` is generic over any tax policy, validates runtime data, returns a concrete `Result`, and reads from `order` without forcing the signature to spell a shared borrow. `main` constructs real domain values and handles the fallible quote path explicitly. The compiler and lowering pipeline own the boring borrow and conversion choices.

You also gain a model-first style for application data:

```incan
model Customer:
    id: int
    email: str
    active: bool
```

This is a compiler-visible shape. It is not a `HashMap`, not a serde-only convention, and not a bag of runtime keys. The field set and field types are facts the compiler can check while you refactor.

And you keep familiar explicit failure:

```incan
def read_config(path: Path) -> Result[Config, ConfigError]:
    text = path.read_text()?
    return Config.parse(text)?
```

Rust's `Result` exists for recoverable errors where a call might succeed or fail and needs to return either success data or error information.[^rust-result] Incan keeps that shape because it is good API design. It does not hide failure in exceptions just to look lighter.

## What you do not lose

Incan is not asking Rust users to give up static structure in exchange for nicer syntax.

You still get:

- compile-time checking for names, fields, signatures, and trait obligations
- `Result`, `Option`, and `?` for explicit fallible code
- traits for named capabilities
- `const` for compile-time facts
- `static` for intentional module-lifetime mutable storage
- models, classes, enums, newtypes, derives, and checked conversions
- Rust crates and Rust-backed standard-library implementation where that is the right lower layer

Incan's claim is narrower and stronger than "easier Rust": it is for cases where you want Rust-shaped confidence but do not want Rust-shaped ceremony to dominate the application layer.

## Where Rust-shaped details still matter

Incan should be able to live in systems-adjacent code too. The question is not whether Incan is allowed near low-level or performance-sensitive work. The question is which details belong in the Incan-facing API and which details belong below the boundary.

Some domains still expose Rust-shaped pressure:

- low-level memory layout
- tight ownership choreography
- lifetime-heavy APIs
- unsafe abstractions
- macro-heavy libraries
- performance-critical code where hand-shaped borrowing and allocation are part of the design
- public APIs meant primarily for Rust consumers

Those are not reasons for Incan to disappear. They are reasons to design the boundary deliberately. A Rust-backed type can expose an Incan-shaped model, trait, or helper API. A hot loop can live under a small Rust implementation while the calling layer stays in Incan. A public Rust crate can offer a Rust-native API and still provide an Incan-friendly surface above it. Unsafe code can remain encapsulated behind a checked, typed Incan contract.

Incan should not pretend these pressures disappear. It should make them show up where they matter, not everywhere by default.

## Where the boundary shows

Rust interop is not merely an escape hatch where Rust leaks through. A lot of the point of Incan is that it can absorb Rust-shaped APIs and present them as ordinary Incan code. You can call common Rust APIs without turning ordinary Incan signatures into `&str`, `&T`, `&mut T`, or lifetime puzzles; the compiler can adapt an Incan `str` into a borrowed `&str` at the Rust boundary for you.[^incan-rust-interop] That is not a special trick; it is the expected shape of the language.

The same idea applies beyond strings. Trait-heavy, lifetime-heavy, unsafe, or macro-heavy Rust APIs are not automatically outside Incan's reach. Incan can often hide those details behind typed imports, Rust-backed types, generated adapters, or small wrapper APIs that expose the useful capability without forcing the whole program to speak Rust's lowest-level vocabulary. When a wrapper is needed, it should be understood as part of good boundary design, not as evidence that Incan cannot live there.

## The right comparison

The right comparison is not "Could I write this in Rust?"

The better comparison is:

- Do I want this layer to expose ownership and lifetime spelling as part of its everyday authoring surface?
- Is this code mostly application modeling, validation, orchestration, data movement, CLI logic, API glue, or domain behavior?
- Would Rust interop give me the performance and ecosystem pieces without forcing the whole layer to be Rust?
- Is a small amount of extra compile work worth a large reduction in authoring ceremony?

If the answer is yes, Incan is worth considering on its own terms.

## See also

- [Why not just Rust?](why_not_just_rust.md)
- [How Incan works](how_incan_works.md)
- [Rust interop](../how-to/rust_interop.md)
- [Models and classes](models_and_classes/index.md)
- [Derives and traits](derives_and_traits.md)
- [Duckborrowing](../../contributing/explanation/duckborrowing.md)

[^rust-ownership]: The Rust Book: [What Is Ownership?](https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html).
[^rust-borrowing]: The Rust Book: [References and Borrowing](https://doc.rust-lang.org/book/ch04-02-references-and-borrowing.html).
[^rust-traits]: The Rust Book: [Defining Shared Behavior with Traits](https://doc.rust-lang.org/book/ch10-02-traits.html).
[^rust-result]: The Rust Book: [Recoverable Errors with `Result`](https://doc.rust-lang.org/book/ch09-02-recoverable-errors-with-result.html).
[^incan-duckborrowing]: Incan contributor docs: [Duckborrowing](../../contributing/explanation/duckborrowing.md).
[^incan-rust-interop]: Incan docs: [Rust interop](../how-to/rust_interop.md).
