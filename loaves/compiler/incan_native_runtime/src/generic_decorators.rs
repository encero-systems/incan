//! Initialize closed decorated callables once, and register declaration names independently of their signatures.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard, PoisonError};
use std::thread::{self, ThreadId};

use incan_std_primitives::errors::raise_value_error;

/// A published instance, or the thread currently publishing it.
#[derive(Clone, Copy)]
enum CacheBinding {
    Initializing(ThreadId),
    Ready,
}

/// The locked lookup decision, returned without retaining the guard across the caller's error reporting.
enum ClaimDecision {
    Ready,
    Acquired,
    Recursive,
    Waiting,
    Poisoned,
}

/// One closed instance publisher's cell: short state changes serialize, independent instances initialize concurrently.
#[derive(Clone)]
struct BindingCache {
    values: Arc<Mutex<HashMap<String, CacheBinding>>>,
    changed: Arc<Condvar>,
}

/// Every compiled unit's publisher address selects its own cell, so two units instantiating one declaration each
/// publish their own storage.
static CACHES: LazyLock<Mutex<Vec<(usize, BindingCache)>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Dynamic callable names, scoped by declaring module: `(module, address, name)`.
static NAMES: LazyLock<Mutex<Vec<(String, usize, String)>>> = LazyLock::new(|| Mutex::new(Vec::new()));

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Replace an equal pointer's name only inside its declaring module.
fn record_name(module: String, address: usize, name: String) {
    let mut names = locked(&NAMES);
    match names
        .iter_mut()
        .find(|(owner, pointer, _)| *owner == module && *pointer == address)
    {
        Some(entry) => entry.2 = name,
        None => names.push((module, address, name)),
    }
}

/// Return a shared cell before any initializer runs, permitting nested calls to another instance.
fn select_cache(publisher: usize) -> BindingCache {
    let mut caches = locked(&CACHES);
    if let Some((_, cell)) = caches.iter().find(|(owner, _)| *owner == publisher) {
        return cell.clone();
    }
    let cell = BindingCache {
        values: Arc::new(Mutex::new(HashMap::new())),
        changed: Arc::new(Condvar::new()),
    };
    caches.push((publisher, cell.clone()));
    cell
}

/// Register a declaration and publish its closed callable once, never holding a lock across user code.
///
/// `publisher` is the address of `publish` and identifies this instance's storage within its compiled unit. A panic
/// while publishing removes the claim before it resumes, so no other caller waits on an abandoned claim.
pub fn initialize_binding(
    module: String,
    instance: String,
    publish: fn(),
    publisher: usize,
    original: usize,
    name: String,
) {
    record_name(module, original, name);
    let cell = select_cache(publisher);
    if !claim_binding(&cell, &instance) {
        return;
    }
    match std::panic::catch_unwind(publish) {
        Ok(()) => finish_binding(&cell, instance, true),
        Err(payload) => {
            finish_binding(&cell, instance, false);
            std::panic::resume_unwind(payload);
        }
    }
}

/// Wait for an existing owner or claim an absent instance; return whether this caller must publish it.
fn claim_binding(cell: &BindingCache, key: &String) -> bool {
    let owner = thread::current().id();
    loop {
        match claim_once(cell, key, owner) {
            ClaimDecision::Ready => return false,
            ClaimDecision::Acquired => return true,
            ClaimDecision::Recursive => {
                raise_value_error(&format!("recursive initialization of decorated callable '{key}'"))
            }
            ClaimDecision::Waiting => {}
            ClaimDecision::Poisoned => raise_value_error(&format!(
                "poisoned decorator cache while another thread initializes '{key}'"
            )),
        }
    }
}

/// Inspect under one guard, waiting atomically for another owner and releasing the guard before returning.
fn claim_once(cell: &BindingCache, key: &String, owner: ThreadId) -> ClaimDecision {
    let mut guard = locked(&cell.values);
    match inspect_claim(&mut guard, key, owner) {
        ClaimDecision::Waiting => match cell.changed.wait(guard) {
            Ok(_) => ClaimDecision::Waiting,
            Err(error) => {
                let mut poisoned = error.into_inner();
                match inspect_claim(&mut poisoned, key, owner) {
                    ClaimDecision::Waiting => ClaimDecision::Poisoned,
                    recovered => recovered,
                }
            }
        },
        decision => decision,
    }
}

/// Inspect or claim while borrowing the caller's guard; another thread's claim requires an atomic wait.
fn inspect_claim(guard: &mut HashMap<String, CacheBinding>, key: &String, owner: ThreadId) -> ClaimDecision {
    match guard.get(key).copied() {
        Some(CacheBinding::Ready) => ClaimDecision::Ready,
        Some(CacheBinding::Initializing(initializer)) if initializer == owner => ClaimDecision::Recursive,
        Some(CacheBinding::Initializing(_)) => ClaimDecision::Waiting,
        None => {
            guard.insert(key.clone(), CacheBinding::Initializing(owner));
            ClaimDecision::Acquired
        }
    }
}

/// Record one published binding or clear its failed claim, then wake callers to recheck the state.
fn finish_binding(cell: &BindingCache, key: String, published: bool) {
    let mut guard = locked(&cell.values);
    if published {
        guard.insert(key, CacheBinding::Ready);
    } else {
        guard.remove(&key);
    }
    cell.changed.notify_all();
}

/// Register the checked declaration name when its function value is materialized, regardless of its signature.
pub fn register_callable_name(module: String, address: usize, name: String) {
    record_name(module, address, name);
}

/// The declaration name registered for this module's function value, or `<callable>` when none was.
pub fn callable_name(module: String, address: usize) -> String {
    let names = locked(&NAMES);
    match names
        .iter()
        .find(|(owner, pointer, _)| *owner == module && *pointer == address)
    {
        Some((_, _, name)) => name.clone(),
        None => "<callable>".to_owned(),
    }
}
