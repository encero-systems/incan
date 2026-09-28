//! Integration tests for the Incan compiler frontend

include!("support/integration_tests_root.rs");

mod parser_tests {
    include!("support/integration_tests_parser_tests.rs");

    #[test]
    fn test_model_with_decorator() {
        let source = r#"
@derive(Debug, Eq)
model User:
  name: str
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Model(m) => {
                assert_eq!(m.decorators.len(), 1);
                assert_eq!(m.decorators[0].node.name, "derive");
            }
            _ => panic!("Expected model"),
        }
    }

    #[test]
    fn test_class_with_traits() {
        let source = r#"
class Service with Loggable, Serializable:
  name: str
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Class(c) => {
                assert_eq!(c.traits.len(), 2);
                assert_eq!(c.traits[0].node.name, "Loggable");
                assert_eq!(c.traits[1].node.name, "Serializable");
            }
            _ => panic!("Expected class"),
        }
    }

    #[test]
    fn test_trait_supertraits_compile_source() {
        let source = r#"
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
"#;

        let result = super::compile_source(source);
        assert!(
            result.is_ok(),
            "expected trait hierarchy program to typecheck, got {:?}",
            result.err()
        );
    }

    #[test]
    fn test_trait_constructor_rejected_in_full_pipeline() {
        let source = r#"
trait Runnable:
  def run(self) -> None: ...

def main() -> None:
  let _r = Runnable()
"#;

        let result = super::compile_source(source);
        let Err(errs) = result else {
            panic!("expected trait construction to fail");
        };
        assert!(
            errs.iter()
                .any(|message| message.contains("Cannot construct trait 'Runnable'")),
            "unexpected errors: {:?}",
            errs
        );
    }

    #[test]
    fn test_method_with_mut_self() {
        let source = r#"
class Counter:
  value: int = 0
  
  def inc(mut self) -> Unit:
    pass
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Class(c) => {
                assert_eq!(c.methods[0].node.receiver, Some(Receiver::Mutable));
            }
            _ => panic!("Expected class"),
        }
    }

    #[test]
    fn test_generic_instance_method_full_pipeline() {
        let source = r#"
class Box:
  def get[T with Clone](self, value: T) -> T:
    return value

model Shelf[U]:
  item: U

  def swap[T with Clone](self, value: T) -> T:
    return value

trait Echo:
  def echo[T with Clone](self, value: T) -> T:
    return value

class EchoBox with Echo:
  marker: int

type Wrapper[U] = newtype U:
  def echo[T with Clone](self, value: T) -> T:
    return value

def main() -> None:
  let b = Box()
  let _x = b.get(1)
  let shelf = Shelf(item=1)
  let _y = shelf.swap("ok")
  let echo = EchoBox(marker=1)
  let _z = echo.echo(True)
  let wrapper = Wrapper(1)
  let _w = wrapper.echo(1.5)
"#;
        let result = super::compile_source(source);
        assert!(
            result.is_ok(),
            "expected generic instance methods across owner kinds to typecheck and lower, got {:?}",
            result.err()
        );
    }

    #[test]
    fn test_issue388_generic_type_owned_factories_run() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
@derive(Clone)
class FactoryBox[T with Clone]:
  pub value: T

  @classmethod
  def make(cls, value: T) -> Self:
    return cls(value=value)

  @staticmethod
  def make_static(value: T) -> Self:
    return FactoryBox(value=value)

def main() -> None:
  from_classmethod = FactoryBox[int].make(1)
  from_staticmethod = FactoryBox[int].make_static(2)
  println(str(from_classmethod.value))
  println(str(from_staticmethod.value))
"#;
        let output = super::incan_command()
            .args(["run", "-c", source])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected generic type-owned factories to run.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr
        );
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["1", "2"],
            "unexpected generic type-owned factory output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_match_with_case() {
        let source = r#"
def foo(x: Option[int]) -> int:
  match x:
    case Some(n):
      return n
    case None:
      return 0
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Function(f) => {
                assert_eq!(f.body.len(), 1);
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_list_comprehension() {
        let source = r#"
def squares(nums: List[int]) -> List[int]:
  return [x * x for x in nums if x > 0]
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        assert_eq!(program.declarations.len(), 1);
    }

    #[test]
    fn test_generic_type() {
        let source = r#"
def foo() -> Result[int, str]:
  return Ok(42)
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Function(f) => match &f.return_type.node {
                Type::Generic(name, args) => {
                    assert_eq!(name, "Result");
                    assert_eq!(args.len(), 2);
                }
                _ => panic!("Expected generic type"),
            },
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_yield_expression() {
        let source = r#"
def fixture() -> str:
  value = "test"
  yield value
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Function(f) => {
                assert_eq!(f.body.len(), 2);
                // Second statement should be the yield
                match &f.body[1].node {
                    Statement::Expr(expr) => {
                        match &expr.node {
                            Expr::Yield(Some(_)) => {} // Success
                            _ => panic!("Expected yield expression with value"),
                        }
                    }
                    _ => panic!("Expected expression statement"),
                }
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_fixture_decorator() {
        let source = r#"
from std.testing import fixture

@fixture(scope="module")
def database() -> Database:
  db = connect()
  yield db
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        // declarations[0] is the import, declarations[1] is the function
        match &program.declarations[1].node {
            Declaration::Function(f) => {
                assert_eq!(f.decorators.len(), 1);
                assert_eq!(f.decorators[0].node.name, "fixture");
                assert!(!f.decorators[0].node.args.is_empty());
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_rust_crate_import() {
        let source = r#"import rust::serde_json as json"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Import(i) => {
                match &i.kind {
                    ImportKind::RustCrate {
                        crate_name,
                        path,
                        version,
                        features,
                    } => {
                        assert_eq!(crate_name, "serde_json");
                        assert!(path.is_empty());
                        assert!(version.is_none());
                        assert!(features.is_empty());
                    }
                    _ => panic!("Expected RustCrate import kind"),
                }
                assert_eq!(i.alias.as_deref(), Some("json"));
            }
            _ => panic!("Expected import"),
        }
    }

    #[test]
    fn test_rust_from_import() {
        let source = r#"from rust::time import Instant, Duration"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Import(i) => match &i.kind {
                ImportKind::RustFrom {
                    crate_name,
                    path,
                    version,
                    features,
                    items,
                } => {
                    assert_eq!(crate_name, "time");
                    assert!(path.is_empty());
                    assert!(version.is_none());
                    assert!(features.is_empty());
                    assert_eq!(items.len(), 2);
                    assert_eq!(items[0].name, "Instant");
                    assert_eq!(items[1].name, "Duration");
                }
                _ => panic!("Expected RustFrom import kind"),
            },
            _ => panic!("Expected import"),
        }
    }
}
