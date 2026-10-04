//! Enums of a `pub::` dependency that share variant names, published the way a library build publishes them and used
//! by a consumer built against the dependency's generated crate and run by rustc.

use super::dependency_references::run_consumer;
use super::packages::TestResult;

/// A value enum and payload enums that share variant names, declared in both orders: each enum publishes its own
/// variants' payloads.
const FLOWS: &str = r#"
@derive(Clone, Eq)
pub enum ActionKind(str):
    Enter = "enter"
    Leave = "leave"


@derive(Clone)
pub model EnterPayload:
    pub step: str


@derive(Clone)
pub enum Action:
    Enter(EnterPayload)
    Leave


@derive(Clone)
pub enum Move:
    Enter(int, int)
    Leave(str)


@derive(Clone)
pub enum Signal:
    Start(int)
    Stop


@derive(Clone, Eq)
pub enum SignalKind(str):
    Start = "start"
    Stop = "stop"


pub def kind_of(action: Action) -> ActionKind:
    match action:
        Action.Enter(_) => return ActionKind.Enter
        Action.Leave => return ActionKind.Leave


pub def describe(step: Move) -> str:
    match step:
        Move.Enter(x, y) => return f"enter {x},{y}"
        Move.Leave(reason) => return f"leave {reason}"


pub def signal_kind(signal: Signal) -> SignalKind:
    match signal:
        Signal.Start(_) => return SignalKind.Start
        Signal.Stop => return SignalKind.Stop
"#;

/// #1561: an enum whose variant shares its name with another enum's variant publishes its own payload. The library
/// used to publish `Action.Enter` with the payload of whichever `Enter` the module bound first (`ActionKind.Enter`,
/// none), so its build failed with "checked/emitted public type arity differs: 0 versus 1"; declared the other way
/// round, `SignalKind.Start` published `Signal.Start`'s payload.
#[test]
fn enums_sharing_a_variant_name_publish_each_enums_own_payload_issue1561() -> TestResult {
    let consumer = r#"
from pub::flows import Action, ActionKind, EnterPayload, Move, Signal, SignalKind, describe, kind_of, signal_kind


def label(action: Action) -> str:
    match action:
        Action.Enter(payload) => return f"enter {payload.step}"
        Action.Leave => return "leave"


def main() -> None:
    enter = Action.Enter(EnterPayload(step="intro"))
    println(label(enter))
    println(label(Action.Leave))
    println(kind_of(enter).value())
    println(kind_of(Action.Leave).value())
    println(ActionKind.Enter.value())
    println(describe(Move.Enter(1, 2)))
    println(describe(Move.Leave("done")))
    println(signal_kind(Signal.Start(3)).value())
    println(SignalKind.Start.value())
    println(SignalKind.Stop.value())
    match Signal.Start(4):
        Signal.Start(value) => println(f"start {value}")
        Signal.Stop => println("stop")
"#;
    assert_eq!(
        run_consumer("flows", FLOWS, consumer)?,
        "enter intro\nleave\nenter\nleave\nenter\nenter 1,2\nleave done\nstart\nstart\nstop\nstart 4\n"
    );
    Ok(())
}

/// #1561: a variant whose payload type the library declares after the enum publishes that type, so a consumer builds
/// the variant from the type and reads the payload's fields. The library used to publish the payload as a type
/// parameter named after the type.
#[test]
fn a_variant_payload_declared_after_its_enum_publishes_its_type_issue1561() -> TestResult {
    let provider = r#"
@derive(Clone)
pub enum Wrapper:
    Holds(Later)
    Empty


@derive(Clone)
pub model Later:
    pub count: int


pub def count_of(wrapper: Wrapper) -> int:
    match wrapper:
        Wrapper.Holds(later) => return later.count
        Wrapper.Empty => return 0
"#;
    let consumer = r#"
from pub::wrappers import Later, Wrapper, count_of


def main() -> None:
    held = Wrapper.Holds(Later(count=2))
    println(count_of(held))
    println(count_of(Wrapper.Empty))
    match held:
        Wrapper.Holds(later) => println(later.count + 1)
        Wrapper.Empty => println("empty")
"#;
    assert_eq!(run_consumer("wrappers", provider, consumer)?, "2\n0\n3\n");
    Ok(())
}
