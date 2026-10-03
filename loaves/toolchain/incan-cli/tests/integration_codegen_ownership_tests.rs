#![allow(dead_code)] // Split test binaries intentionally use different parts of shared support fragments.

//! Integration tests for the Incan compiler frontend

include!("support/integration_tests_root.rs");

mod codegen_tests {
    include!("support/integration_tests_codegen_tests.rs");

    #[test]
    fn test_comprehension_and_generator_execution_matrix() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
model StoredNode:
    store_id_raw: int
    node: str


def filtered_comprehension() -> None:
    nodes: list[StoredNode] = [
        StoredNode(store_id_raw=1, node="a"),
        StoredNode(store_id_raw=2, node="b"),
    ]
    filtered = [stored.node for stored in nodes if stored.store_id_raw == 1]
    scores = [1, 2, 3, 4]
    squared_evens = {x: x * x for x in scores if x % 2 == 0}
    println(filtered[0])
    println(squared_evens[2])


def source_ordered_generator_expression() -> None:
    xs = [1, 2, 3]
    ys = [2, 3, 4]
    values = (x * y for x in xs if x > 1 for y in ys if y > x).collect()
    println(values[0])
    println(values[1])
    println(values[2])


def triple(x: int) -> int:
    return x * 3

def big(x: int) -> bool:
    return x > 6


def generator_helper_chain() -> None:
    xs = [1, 2, 3, 4, 5]
    values = (x for x in xs).map(triple).filter(big).take(2).collect()
    println(values[0])
    println(values[1])


def numbers() -> Generator[int]:
    yield 1
    yield 2


def concrete_generator_yield() -> None:
    values = numbers().collect()
    println(values[0])
    println(values[1])


def lazy_numbers() -> Generator[int]:
    println("started")
    yield 1


def lazy_generator_body() -> None:
    values = lazy_numbers()
    println("after construction")
    items = values.collect()
    println(items[0])


def singleton[T](value: T) -> Generator[T]:
    yield value


def generic_generator_yield() -> None:
    values = singleton[int](3).collect()
    println(values[0])


def main() -> None:
    filtered_comprehension()
    source_ordered_generator_expression()
    generator_helper_chain()
    concrete_generator_yield()
    lazy_generator_body()
    generic_generator_yield()
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "comprehension and generator execution matrix failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec![
                "a",
                "4",
                "6",
                "8",
                "12",
                "9",
                "12",
                "1",
                "2",
                "after construction",
                "started",
                "1",
                "3",
            ],
            "unexpected comprehension/generator matrix output:\n{stdout}"
        );
        Ok(())
    }

    /// #1123 regression: generator-expression construction evaluates the outer source once, but every filter and
    /// element evaluation remains deferred until a consumer polls the generator. This is a generated-Rust
    /// compile-and-run oracle for the established source contract; it does not claim replacement-executor support.
    #[test]
    fn test_generator_expression_defers_filter_and_element_evaluation() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
def source() -> list[int]:
    println("outer source")
    return [1, 2, 3]


def keep(value: int) -> bool:
    println("filter")
    return value > 1


def project(value: int) -> int:
    println("element")
    return value * 10


def main() -> None:
    values = (project(value) for value in source() if keep(value))
    println("after construction")
    collected = values.collect()
    println(collected[0])
    println(collected[1])
"#;
        let output = run_incan_source(source);
        assert!(
            output.status.success(),
            "generator-expression laziness regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec![
                "outer source",
                "after construction",
                "filter",
                "filter",
                "element",
                "filter",
                "element",
                "20",
                "30",
            ],
            "generator-expression filters/elements must not run before construction completes:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_clone_self_struct_field_reads_do_not_move_out_of_borrowed_self() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
pub class ActiveRegistration:
    pub logical_name: str
    pub rank: int

    def clone(self) -> Self:
        return ActiveRegistration(logical_name=self.logical_name, rank=self.rank)

def main() -> None:
    reg = ActiveRegistration(logical_name="orders", rank=1)
    copied = reg.clone()
    println(copied.logical_name)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run -c clone(self)->Self field regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["orders"], "unexpected clone(self)->Self output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_loop_item_field_index_assignment_materializes_owned_value_issue616()
    -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
model Assignment:
    output_name: str

def names(assignments: list[Assignment]) -> list[str]:
    mut output_names: list[str] = []
    for assignment in assignments:
        existing_idx = index_of_name(output_names, assignment.output_name)
        if existing_idx >= 0:
            output_names[existing_idx] = assignment.output_name
        else:
            output_names.append(assignment.output_name)
    return output_names

def index_of_name(names: list[str], name: str) -> int:
    for idx, current in enumerate(names):
        if current == name:
            return idx
    return -1

def main() -> None:
    result = names([Assignment(output_name="amount"), Assignment(output_name="amount")])
    println(result[0])
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "loop item field index-assignment regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["amount"],
            "unexpected loop item field index-assignment output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_field_backed_by_value_method_args_do_not_require_user_clone_issue241()
    -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
class Cursor:
    def join(self, other: Self, on: bool) -> Self:
        return Cursor()

@derive(Clone)
class Wrapper:
    _cursor: Cursor

    def merge(self, other: Self) -> Self:
        return Wrapper(_cursor=self._cursor.join(other._cursor, true))

def main() -> None:
    left = Wrapper(_cursor=Cursor())
    right = Wrapper(_cursor=Cursor())
    _ = left.merge(right)
    println("ok")
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "field-backed by-value method arg regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["ok"], "unexpected issue241 output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_issue241_generic_field_backed_method_args_infer_clone_bounds() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
class Cursor[T]:
    pub value: T

    def join(self, other: Self, on: bool) -> Self:
        return self

@derive(Clone)
class Wrapper[T]:
    pub _cursor: Cursor[T]

    def merge(self, other: Self) -> Self:
        return Wrapper(_cursor=self._cursor.join(other._cursor, true))

def main() -> None:
    left = Wrapper(_cursor=Cursor(value=1))
    right = Wrapper(_cursor=Cursor(value=2))
    println(left.merge(right)._cursor.value)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "generic issue241 regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["1"], "unexpected generic issue241 output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_returning_tuple_with_reused_field_materializes_owned_items() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
class Pred:
    pub name: str

@derive(Clone)
class Node:
    pub filter_predicate: Pred

def pair(node: Node) -> tuple[Pred, Pred]:
    return (node.filter_predicate, node.filter_predicate)

def main() -> None:
    left, right = pair(Node(filter_predicate=Pred(name="x")))
    println(left.name)
    println(right.name)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "tuple field reuse ownership regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["x", "x"], "unexpected tuple field reuse output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_generic_tuple_return_with_reused_field_infers_clone_bound() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
class Node[T]:
    pub value: T

def pair[T](node: Node[T]) -> tuple[T, T]:
    return (node.value, node.value)

def main() -> None:
    left, right = pair(Node(value=1))
    println(left)
    println(right)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "generic tuple field reuse regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["1", "1"],
            "unexpected generic tuple field reuse output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_incan_call_materializes_owned_value_from_box_as_ref() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
from rust::std::boxed import Box

@derive(Clone)
class Node:
    pub value: int

def take(node: Node) -> int:
    return node.value

def from_box(child: Box[Node]) -> int:
    return take(child.as_ref())

def main() -> None:
    println(from_box(Box.new(Node(value=4))))
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "borrowed box as_ref call regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["4"], "unexpected box as_ref output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_generic_incan_call_materializes_owned_value_from_box_as_ref() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
from rust::std::boxed import Box

@derive(Clone)
class Node[T]:
    pub value: T

def take[T](node: Node[T]) -> T:
    return node.value

def from_box[T](child: Box[Node[T]]) -> T:
    return take(child.as_ref())

def main() -> None:
    println(from_box(Box.new(Node(value=4))))
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "generic borrowed box as_ref call regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["4"], "unexpected generic box as_ref output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_match_on_shared_self_option_field_materializes_owned_scrutinee() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
pub class Node:
    pub value: int

@derive(Clone)
pub class Wrapper:
    child: Option[Node]

    def read(self) -> int:
        match self.child:
            Some(child) => return child.value
            None => return 0

def main() -> None:
    println(Wrapper(child=Some(Node(value=4))).read())
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "shared self option-field match regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["4"],
            "unexpected shared self option-field match output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_match_on_shared_self_option_box_field_materializes_owned_scrutinee()
    -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
from rust::std::boxed import Box

@derive(Clone)
pub class Node:
    pub value: int

@derive(Clone)
pub class Wrapper:
    child: Option[Box[Node]]

    def read(self) -> int:
        match self.child:
            Some(child) => return child.as_ref().value
            None => return 0

def main() -> None:
    println(Wrapper(child=Some(Box.new(Node(value=4)))).read())
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "shared self option-box-field match regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["4"],
            "unexpected shared self option-box-field match output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_generic_match_on_shared_self_option_field_infers_clone_bound() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
pub class Wrapper[T]:
    child: Option[T]

    def read_or(self, fallback: T) -> T:
        match self.child:
            Some(child) => return child
            None => return fallback

def main() -> None:
    println(Wrapper(child=Some(4)).read_or(0))
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "generic shared self option-field match regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["4"],
            "unexpected generic shared self option-field match output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_trait_supertraits_runtime_with_backend_clone_bounds() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
trait Collection[T]:
    def first(self) -> T: ...

trait OrderedCollection[T] with Collection[T]:
    def sorted(self) -> Self: ...

model BoxedValue[T] with OrderedCollection:
    value: T

    def first(self) -> T:
        return self.value

    def sorted(self) -> Self:
        return self

def take_first(values: Collection[int]) -> int:
    return values.first()

def take_sorted(values: OrderedCollection[int]) -> OrderedCollection[int]:
    return values.sorted()

def main() -> None:
    println(take_first(BoxedValue(value=1)))
    println(take_sorted(BoxedValue(value=2)).first())
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "trait-supertrait ownership regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["1", "2"], "unexpected trait-supertrait output:\n{stdout}");
        Ok(())
    }
}
