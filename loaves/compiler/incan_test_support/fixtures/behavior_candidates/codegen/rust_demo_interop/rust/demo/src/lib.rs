//! The `demo` crate the retired `codegen.rs` tests described through synthetic inspection metadata, made real.

/// A variant whose payload is boxed: every argument shape must reach `Box::new`.
pub enum Kind {
    Tuple(Box<i64>),
}

impl Kind {
    /// Read the boxed payload back.
    pub fn payload(&self) -> i64 {
        match self {
            Kind::Tuple(value) => **value,
        }
    }
}

/// A handle-like trait standing in for `AsFd`.
pub trait Handle {
    /// The handle's number.
    fn number(&self) -> i64;
}

/// A file-like value that is a `Handle`.
pub struct File {
    pub number: i64,
}

impl Handle for File {
    /// The file number.
    fn number(&self) -> i64 {
        self.number
    }
}

/// Borrow any handle, like `flock(&impl AsFd)`.
pub fn flock(handle: &impl Handle) -> i64 {
    handle.number()
}

/// A generic factory whose `new` is owner-specialized.
pub struct PairFactory<T, U> {
    pub first: T,
    pub second: U,
}

impl<T, U> PairFactory<T, U> {
    /// Build a pair from its two values.
    pub fn new(first: T, second: U) -> PairFactory<T, U> {
        PairFactory { first, second }
    }
}

/// A named-field struct constructed positionally or by name from Incan.
pub struct Pair {
    pub zeta: i64,
    pub alpha: i64,
}

/// A color with three channels.
pub struct Color {
    pub red: f32,
    pub green: f32,
    pub blue: f32,
}

impl Color {
    /// Build a color from sRGB channels.
    pub fn srgb(red: f32, green: f32, blue: f32) -> Color {
        Color { red, green, blue }
    }
}

/// A tuple struct wrapping a color.
pub struct ClearColor(pub Color);

/// A command buffer that is passed by value and mutated.
pub struct Commands {
    pub spawned: i64,
}

impl Commands {
    /// Start with nothing spawned.
    pub fn new() -> Commands {
        Commands { spawned: 0 }
    }

    /// Spawn an empty entity.
    pub fn spawn_empty(&mut self) {
        self.spawned += 1;
    }
}

impl Default for Commands {
    /// The `new` value.
    fn default() -> Self {
        Commands::new()
    }
}

/// A leaf mutated through a projected `&mut`.
pub struct Widget {
    pub value: i64,
}

/// Another leaf mutated through a projected `&mut`.
pub struct Gadget {
    pub value: i64,
}

/// A leaf that stays owned.
pub struct Entity {
    pub id: i64,
}

/// A container whose type argument is projected to mutable references (a `Query<(&mut A, &mut B)>` shape).
pub struct FooBar<Q> {
    items: Vec<Q>,
}

impl<Q> FooBar<Q> {
    /// An empty container.
    pub fn new() -> FooBar<Q> {
        FooBar { items: Vec::new() }
    }

    /// Iterate the items mutably.
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, Q> {
        self.items.iter_mut()
    }

    /// How many items the container holds.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the container is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl<Q> Default for FooBar<Q> {
    /// The `new` value.
    fn default() -> Self {
        FooBar::new()
    }
}

impl<'a> FooBar<(&'a mut Widget, &'a mut Gadget)> {
    /// A container over one widget and one gadget.
    pub fn pair(widget: &'a mut Widget, gadget: &'a mut Gadget) -> Self {
        FooBar {
            items: vec![(widget, gadget)],
        }
    }
}

/// A container with two projected type arguments.
pub struct FooBar2<A, B> {
    pub first: A,
    pub second: B,
}

/// A generic container that declares no mutable-reference projection.
pub struct PlainBar<Q> {
    pub inner: Q,
}

impl<Q> PlainBar<Q> {
    /// Wrap a value.
    pub fn new(inner: Q) -> PlainBar<Q> {
        PlainBar { inner }
    }
}

/// A component marker trait a derive supplies.
pub trait Component {}

/// A candidate whose associated type the solver accepts.
pub struct Dynamic {
    pub value: i64,
}

/// A candidate whose associated type the solver rejects.
pub struct Static {
    pub value: i64,
}

/// Accept an `f32` untouched.
pub fn accept_f32(value: f32) -> f32 {
    value
}

/// Fields spelled like Rust keywords.
pub struct JoinRel {
    pub r#type: i64,
    pub r#match: i64,
    pub type_: i64,
}

/// A builder-like type whose `json` borrows its payload.
pub struct Builder {
    pub length: usize,
}

impl Builder {
    /// A fresh builder.
    pub fn new() -> Builder {
        Builder { length: 0 }
    }

    /// Record the payload's serialized length.
    pub fn json<T: ?Sized>(&self, _payload: &T) -> i64 {
        1
    }
}

impl Default for Builder {
    /// The `new` value.
    fn default() -> Self {
        Builder::new()
    }
}

/// Options passed as a nested associated call.
pub struct WriteOptions;

impl WriteOptions {
    /// Default options.
    pub fn new() -> WriteOptions {
        WriteOptions
    }
}

impl Default for WriteOptions {
    /// The `new` value.
    fn default() -> Self {
        WriteOptions::new()
    }
}

/// A context whose method takes the nested options.
pub struct SessionContext;

impl SessionContext {
    /// A fresh context.
    pub fn new() -> SessionContext {
        SessionContext
    }

    /// Count the characters of the URI plus the options.
    pub fn write_csv(&self, uri: &str, _options: WriteOptions, limit: Option<i64>) -> i64 {
        uri.len() as i64 + limit.unwrap_or(0)
    }
}

impl Default for SessionContext {
    /// The `new` value.
    fn default() -> Self {
        SessionContext::new()
    }
}

/// A trait whose method is reached through an import (the `message_probe` shape).
pub trait Message {
    /// Encode to bytes.
    fn encode_to_vec(&self) -> Vec<u8>;
}

/// A packet that is a `Message`.
pub struct Packet;

impl Message for Packet {
    /// Three fixed bytes.
    fn encode_to_vec(&self) -> Vec<u8> {
        vec![1, 2, 3]
    }
}

/// A MAC-like engine driven through trait-qualified calls.
#[derive(Default)]
pub struct Engine {
    pub fed: usize,
}

/// The trait whose receivers decide the borrow of a trait-qualified call.
pub trait Mac {
    /// Feed data.
    fn update(&mut self, data: &[u8]);
    /// Reborrow the receiver.
    fn by_ref(&mut self) -> &mut Self;
    /// Whether anything was fed.
    fn is_ready(&self) -> bool;
    /// Finish and return the digest bytes.
    fn finalize(self) -> Vec<u8>;
}

impl Mac for Engine {
    /// Count the fed bytes.
    fn update(&mut self, data: &[u8]) {
        self.fed += data.len();
    }

    /// The receiver itself.
    fn by_ref(&mut self) -> &mut Self {
        self
    }

    /// Whether bytes were fed.
    fn is_ready(&self) -> bool {
        self.fed > 0
    }

    /// One zero byte per fed byte.
    fn finalize(self) -> Vec<u8> {
        vec![0; self.fed]
    }
}

/// An expression evaluated through a closure predicate.
pub struct Expression {
    pub seed: i64,
}

/// The predicate value the closure receives.
pub struct Predicate {
    pub value: i64,
}

impl Expression {
    /// Evaluate the expression by asking the closure about its predicate.
    pub fn eval<F: Fn(&Predicate) -> Option<bool>>(&self, predicate: F) -> Option<bool> {
        predicate(&Predicate { value: self.seed })
    }
}
