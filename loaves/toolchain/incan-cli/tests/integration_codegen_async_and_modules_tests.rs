//! Integration tests for the Incan compiler frontend

include!("support/integration_tests_root.rs");

mod codegen_tests {
    include!("support/integration_tests_codegen_tests.rs");

    #[test]
    fn test_run_file_release_flag() -> Result<(), Box<dyn std::error::Error>> {
        let project_dir = make_temp_dir("incan_run_release_file");
        let source_path = project_dir.join("main.incn");
        std::fs::write(
            &source_path,
            r#"def main() -> None:
  println("release file path works")
"#,
        )?;

        let output = incan_command()
            .args(["run", "--release", source_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run --release <file> failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("release file path works"),
            "stdout missing expected output; got:\n{}",
            stdout
        );
        Ok(())
    }

    #[test]
    fn test_check_web_route_uses_proc_macro_passthrough() {
        let project_dir = make_temp_dir("incan_web_proc_macro_test");
        let source_path = project_dir.join("main.incn");
        let source = r#"
import std.async
from std.web import route

@route("/health")
async def health() -> str:
    return "ok"

def main() -> None:
    pass
"#;
        let Ok(()) = std::fs::write(&source_path, source) else {
            panic!("failed to write source file");
        };

        let Ok(output) = incan_command()
            .args(["--check", source_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan check");
        };

        assert!(
            output.status.success(),
            "incan check web route failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn test_run_async_channel_facade() -> Result<(), Box<dyn std::error::Error>> {
        let project_dir = make_temp_dir("incan_async_channel_facade_test");
        let source_path = project_dir.join("async_channel.incn");
        let source = r#"
import std.async
from std.async.channel import channel, unbounded_channel, oneshot

async def main() -> None:
    tx, rx = channel(4)
    cloned = tx.clone()

    match await cloned.send(1):
        Ok(_) => println("sent")
        Err(err) => println(err.message())

    match await rx.recv():
        Some(value) => println(value)
        None => println("closed")

    match await tx.reserve():
        Ok(permit) =>
            match permit.send(4):
                Ok(_) => println("reserved")
                Err(err) => println(err.message())
        Err(err) => println(err.message())

    match await rx.recv():
        Some(value) => println(value)
        None => println("closed")

    tx2, rx2 = unbounded_channel()
    match await tx2.send(2):
        Ok(_) => println("sent")
        Err(err) => println(err.message())

    match rx2.try_recv():
        Some(value) => println(value)
        None => println("empty")

    match await tx2.reserve():
        Ok(permit) =>
            match permit.send(5):
                Ok(_) => println("unbounded reserved")
                Err(err) => println(err.message())
        Err(err) => println(err.message())

    match rx2.try_recv():
        Some(value) => println(value)
        None => println("empty")

    println(f"close:{rx2.close()}")
    println(tx2.is_closed())

    otx, orx = oneshot()
    match otx.send(3):
        Ok(_) => println("delivered")
        Err(value) => println(value)

    match await orx.recv():
        Ok(value) => println(value)
        Err(err) => println(err.message())
"#;
        std::fs::write(&source_path, source)?;

        let output = incan_command()
            .args(["run", source_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan run async channel facade failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("sent"), "expected send output; got:\n{}", stdout);
        assert!(
            stdout.contains("1"),
            "expected bounded receive output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("2"),
            "expected unbounded receive output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("reserved"),
            "expected bounded reserve output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("4"),
            "expected bounded permit receive output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("unbounded reserved"),
            "expected unbounded reserve output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("5"),
            "expected unbounded permit receive output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("close:true"),
            "expected receiver close output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("true"),
            "expected closed-state output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("delivered"),
            "expected oneshot send output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("3"),
            "expected oneshot receive output; got:\n{}",
            stdout
        );
        Ok(())
    }

    /// Regression (GitHub #289): `await expr?` must emit `.await?` (not `?.await`) in generated Rust.
    #[test]
    fn test_build_async_await_try_ordering_emits_await_before_try() {
        let project_dir = make_temp_dir("incan_async_await_try_ordering");
        let source_path = project_dir.join("async_await_try_ordering.incn");
        let out_dir = project_dir.join("out");
        let source = r#"
import std.async

async def register_sources() -> Result[None, str]:
    return Ok(None)

async def main() -> Result[None, str]:
    await register_sources()?
    return Ok(None)
"#;
        let Ok(()) = std::fs::write(&source_path, source) else {
            panic!("failed to write source file");
        };

        let Ok(output) = incan_command()
            .args([
                "build",
                source_path.to_string_lossy().as_ref(),
                out_dir.to_string_lossy().as_ref(),
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan build");
        };

        assert!(
            output.status.success(),
            "incan build await/try ordering regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let generated_main = out_dir.join("src/main.rs");
        let Ok(main_rs) = std::fs::read_to_string(&generated_main) else {
            panic!("failed to read generated Rust source");
        };
        let normalized: String = main_rs.chars().filter(|c| !c.is_whitespace()).collect();
        // Assert the ordering, not the callee's spelling. RFC 120 projections emit a linker-visible
        // `__incan_v1_...` name for `register_sources`, so pinning the source spelling tested the projection rather
        // than the await/try ordering this case exists for.
        assert!(
            normalized.contains(").await?;"),
            "expected awaited-then-try ordering in generated Rust, got:\n{}",
            main_rs
        );
        assert!(
            !normalized.contains(")?.await"),
            "generated Rust must not apply `?` before `.await`, got:\n{}",
            main_rs
        );
    }

    #[test]
    fn test_build_and_run_keyword_named_modules_escape_consistently() -> Result<(), Box<dyn std::error::Error>> {
        let project_dir = make_temp_dir("incan_keyword_module_paths");
        let src_dir = project_dir.join("src");
        std::fs::create_dir_all(src_dir.join("api"))?;
        std::fs::write(
            project_dir.join("loaf.toml"),
            "[project]\nname = \"keyword_module_paths\"\nversion = \"0.1.0\"\n",
        )?;

        let main_path = src_dir.join("main.incn");
        // Use a Rust keyword that remains a legal Incan module spelling. `type` is a separate Incan keyword, so
        // parser work to allow `from type import ...` would be a different issue than Rust-side module escaping.
        std::fs::write(
            &main_path,
            r#"from extern import root_value
from api.extern import nested_value

def main() -> None:
  println(root_value())
  println(nested_value())
"#,
        )?;
        std::fs::write(
            src_dir.join("extern.incn"),
            r#"pub def root_value() -> str:
  return "root-keyword"
"#,
        )?;
        std::fs::write(
            src_dir.join("api").join("extern.incn"),
            r#"pub def nested_value() -> str:
  return "nested-keyword"
"#,
        )?;

        let out_dir = project_dir.join("out");
        let build_output = incan_command()
            .args([
                "build",
                main_path.to_string_lossy().as_ref(),
                out_dir.to_string_lossy().as_ref(),
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            build_output.status.success(),
            "incan build keyword-module project failed: status={:?} stderr={}",
            build_output.status,
            String::from_utf8_lossy(&build_output.stderr)
        );

        let main_rs = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        let api_mod_rs = std::fs::read_to_string(out_dir.join("src/api/mod.rs"))?;
        let normalized_main: String = main_rs.chars().filter(|c| !c.is_whitespace()).collect();
        let normalized_api_mod: String = api_mod_rs.chars().filter(|c| !c.is_whitespace()).collect();

        assert!(
            normalized_main.contains("#[path=\"extern.rs\"]modr#extern;"),
            "expected top-level keyword module path attr in generated main.rs, got:\n{main_rs}"
        );
        // The escape is `crate::r#extern::`; what follows it is the callee's emitted name, which RFC 120 projections
        // now own. Asserting the raw `root_value` spelling tested the projection instead of the keyword escaping.
        assert!(
            normalized_main.contains("crate::r#extern::"),
            "expected generated use path to escape top-level keyword module, got:\n{main_rs}"
        );
        assert!(
            normalized_main.contains("crate::api::r#extern::"),
            "expected generated use path to escape nested keyword module, got:\n{main_rs}"
        );
        assert!(
            normalized_api_mod.contains("#[path=\"extern.rs\"]pubmodr#extern;"),
            "expected nested keyword module path attr in api/mod.rs, got:\n{api_mod_rs}"
        );

        // The normal build above has already exercised the compiler command and produced this exact executable.
        // Reuse it for the runtime assertion rather than taking the same source through a second compilation path.
        let binary = out_dir.join("oven/release/keyword_module_paths");
        assert!(
            binary.is_file(),
            "expected Oven to produce the keyword-module executable at {}",
            binary.display()
        );
        let run_output = Command::new(&binary).output()?;
        assert!(
            run_output.status.success(),
            "incan run keyword-module project failed: status={:?} stderr={}",
            run_output.status,
            String::from_utf8_lossy(&run_output.stderr)
        );

        let stdout = String::from_utf8_lossy(&run_output.stdout);
        assert!(
            stdout.contains("root-keyword"),
            "expected top-level keyword module output, got:\n{stdout}"
        );
        assert!(
            stdout.contains("nested-keyword"),
            "expected nested keyword module output, got:\n{stdout}"
        );

        Ok(())
    }

    /// Regression (GitHub #976): an explicit `crate::` import must select the root module even when a nested module
    /// has the same leaf name.
    #[test]
    fn test_run_explicit_root_import_over_same_leaf_nested_module() -> Result<(), Box<dyn std::error::Error>> {
        let project_dir = make_temp_dir("incan_explicit_root_import");
        let src_dir = project_dir.join("src");
        let nested_dir = src_dir.join("substrait");
        std::fs::create_dir_all(&nested_dir)?;
        std::fs::write(
            project_dir.join("loaf.toml"),
            "[project]\nname = \"explicit_root_import\"\nversion = \"0.1.0\"\n",
        )?;

        let main_path = src_dir.join("main.incn");
        std::fs::write(
            &main_path,
            r#"from substrait.schema_registry import selected_value

def main() -> None:
  println(selected_value())
"#,
        )?;
        std::fs::write(
            src_dir.join("schema_registry.incn"),
            r#"pub def root_value() -> str:
  return "root registry"
"#,
        )?;
        std::fs::write(
            nested_dir.join("schema_registry.incn"),
            r#"from crate::schema_registry import root_value

pub def selected_value() -> str:
  return root_value()
"#,
        )?;

        let run_output = incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("INCAN_SOURCE_ROOT", repo_root())
            .env("INCAN_STDLIB", repo_root().join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            run_output.status.success(),
            "explicit root import project failed: status={:?} stderr={}",
            run_output.status,
            String::from_utf8_lossy(&run_output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&run_output.stdout).contains("root registry"),
            "explicit root import did not run the root module: stdout={} stderr={}",
            String::from_utf8_lossy(&run_output.stdout),
            String::from_utf8_lossy(&run_output.stderr)
        );

        Ok(())
    }

    /// Regression (GitHub #976): a bare import that resolves to its own nested source file must give root guidance.
    #[test]
    fn test_build_rejects_same_leaf_self_import_with_root_guidance() -> Result<(), Box<dyn std::error::Error>> {
        let project_dir = make_temp_dir("incan_same_leaf_self_import");
        let src_dir = project_dir.join("src");
        let nested_dir = src_dir.join("substrait");
        std::fs::create_dir_all(&nested_dir)?;
        std::fs::write(
            project_dir.join("loaf.toml"),
            "[project]\nname = \"same_leaf_self_import\"\nversion = \"0.1.0\"\n",
        )?;

        let main_path = src_dir.join("main.incn");
        std::fs::write(
            &main_path,
            r#"from substrait.schema_registry import registered_columns

def main() -> None:
  println(registered_columns())
"#,
        )?;
        std::fs::write(
            src_dir.join("schema_registry.incn"),
            r#"pub def registered_columns() -> str:
  return "root registry"
"#,
        )?;
        std::fs::write(
            nested_dir.join("schema_registry.incn"),
            r#"from schema_registry import registered_columns

pub def registered_columns() -> str:
  return registered_columns()
"#,
        )?;

        let output = incan_command()
            .args(["build", main_path.to_string_lossy().as_ref()])
            .env("INCAN_SOURCE_ROOT", repo_root())
            .env("INCAN_STDLIB", repo_root().join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            !output.status.success(),
            "self-importing source module unexpectedly built successfully:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("imports itself"),
            "expected a self-import diagnostic, got:\n{stderr}"
        );
        assert!(
            stderr.contains("crate::schema_registry"),
            "expected root-import guidance, got:\n{stderr}"
        );

        Ok(())
    }

    #[test]
    fn test_run_async_task_and_time_facade() -> Result<(), Box<dyn std::error::Error>> {
        let project_dir = make_temp_dir("incan_async_task_time_facade_test");
        let source_path = project_dir.join("async_task_time.incn");
        let source = r#"
import std.async
from std.async.task import spawn, spawn_blocking
from std.async.time import sleep, timeout, timeout_ms, timeout_join, timeout_join_ms, TimeoutJoinOutcome

async def quick_value() -> int:
    await sleep(0.01)
    return 7

async def slow_value() -> int:
    await sleep(0.05)
    return 99

def blocking_value() -> int:
    return 42

async def main() -> None:
    match await spawn(quick_value()):
        Ok(value) => println(f"spawn_ok:{value}")
        Err(err) => println(f"spawn_err:{err.message()}")

    match await spawn_blocking(blocking_value):
        Ok(value) => println(f"spawn_blocking_ok:{value}")
        Err(err) => println(f"spawn_blocking_err:{err.message()}")

    match await timeout(0.25, quick_value()):
        Ok(value) => println(f"timeout_ok:{value}")
        Err(err) => println(f"timeout_err:{err.message()}")

    match await timeout(0.001, slow_value()):
        Ok(value) => println(f"timeout_unexpected_ok:{value}")
        Err(err) => println(f"timeout_expired:{err.message()}")

    match await timeout_ms(250, quick_value()):
        Ok(value) => println(f"timeout_ms_ok:{value}")
        Err(err) => println(f"timeout_ms_err:{err.message()}")

    match await timeout_ms(1, slow_value()):
        Ok(value) => println(f"timeout_ms_unexpected_ok:{value}")
        Err(err) => println(f"timeout_ms_expired:{err.message()}")

    durable = spawn(slow_value())
    match await timeout_join(0.001, durable):
        TimeoutJoinOutcome.Completed(value) => println(f"timeout_join_unexpected_ok:{value}")
        TimeoutJoinOutcome.JoinFailed(err) => println(f"timeout_join_err:{err.message()}")
        TimeoutJoinOutcome.TimedOut(handle) =>
            println("task still running after timeout")
            match await handle:
                Ok(value) => println(f"timeout_join_later:{value}")
                Err(err) => println(f"timeout_join_later_err:{err.message()}")

    durable_ms = spawn(slow_value())
    match await timeout_join_ms(1, durable_ms):
        TimeoutJoinOutcome.Completed(value) => println(f"timeout_join_ms_unexpected_ok:{value}")
        TimeoutJoinOutcome.JoinFailed(err) => println(f"timeout_join_ms_err:{err.message()}")
        TimeoutJoinOutcome.TimedOut(handle) =>
            match await handle:
                Ok(value) => println(f"timeout_join_ms_later:{value}")
                Err(err) => println(f"timeout_join_ms_later_err:{err.message()}")
"#;
        std::fs::write(&source_path, source)?;

        let output = incan_command()
            .args(["run", source_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan run async task/time facade failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("spawn_ok:7"),
            "expected spawn success output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("spawn_blocking_ok:42"),
            "expected spawn_blocking success output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("timeout_ok:7"),
            "expected timeout success output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("timeout_expired:operation timed out"),
            "expected timeout expiry output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("timeout_ms_ok:7"),
            "expected timeout_ms success output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("timeout_ms_expired:operation timed out"),
            "expected timeout_ms expiry output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("task still running after timeout"),
            "expected durable timeout message; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("timeout_join_later:99"),
            "expected timeout_join preserved handle output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("timeout_join_ms_later:99"),
            "expected timeout_join_ms preserved handle output; got:\n{}",
            stdout
        );
        assert!(
            !stdout.contains("timeout_unexpected_ok")
                && !stdout.contains("timeout_ms_unexpected_ok")
                && !stdout.contains("timeout_join_unexpected_ok")
                && !stdout.contains("timeout_join_ms_unexpected_ok")
                && !stdout.contains("spawn_err:")
                && !stdout.contains("spawn_blocking_err:")
                && !stdout.contains("timeout_err:")
                && !stdout.contains("timeout_ms_err:"),
            "unexpected error/success fallback branch output; got:\n{}",
            stdout
        );
        Ok(())
    }

    #[test]
    fn test_run_async_barrier_cancellation_withdraws_waiter() -> Result<(), Box<dyn std::error::Error>> {
        let project_dir = make_temp_dir("incan_async_barrier_cancel_test");
        let source_path = project_dir.join("async_barrier_cancel.incn");
        let source = r#"
import std.async
from std.async.sync import Barrier, Mutex
from std.async.task import spawn, yield_now
from std.async.time import timeout_join_ms, TimeoutJoinOutcome

async def mark_ready(ready: Mutex[int]) -> None:
    guard = await ready.lock()
    guard.set(1)

async def is_ready(ready: Mutex[int]) -> bool:
    guard = await ready.lock()
    return guard.get() == 1

async def wait_until_ready(ready: Mutex[int]) -> None:
    while True:
        if await is_ready(ready):
            return
        await yield_now()

async def wait_barrier(barrier: Barrier, ready: Mutex[int]) -> int:
    await mark_ready(ready)
    return await barrier.wait()

async def main() -> None:
    barrier = Barrier.new(2)

    cancelled_ready = Mutex.new(0)
    cancelled = spawn(wait_barrier(barrier, cancelled_ready))
    await wait_until_ready(cancelled_ready)
    cancelled.abort()
    match await cancelled:
        Ok(slot) => println(f"unexpected_cancelled_slot:{slot}")
        Err(err) => println(f"cancelled:{err.message()}")

    replacement_ready = Mutex.new(0)
    replacement = spawn(wait_barrier(barrier, replacement_ready))
    await wait_until_ready(replacement_ready)
    match await timeout_join_ms(5, replacement):
        TimeoutJoinOutcome.Completed(slot) => println(f"unexpected_replacement_completed:{slot}")
        TimeoutJoinOutcome.JoinFailed(err) => println(f"unexpected_replacement_failed:{err.message()}")
        TimeoutJoinOutcome.TimedOut(handle) =>
            println("replacement_waiting")
            current = await barrier.wait()
            match await handle:
                Ok(slot) => println(f"replacement_slot:{slot}")
                Err(err) => println(f"unexpected_replacement_join_failed:{err.message()}")
            println(f"current_slot:{current}")
"#;
        std::fs::write(&source_path, source)?;

        let output = incan_command()
            .args(["run", source_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan run async barrier cancellation failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("cancelled:task") && stdout.contains("was cancelled"),
            "expected cancelled join output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("replacement_waiting"),
            "expected replacement to keep waiting until another active participant arrived; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("replacement_slot:") && stdout.contains("current_slot:"),
            "expected both active participants to complete after the second arrival; got:\n{}",
            stdout
        );
        assert!(
            !stdout.contains("unexpected_"),
            "unexpected fallback branch output; got:\n{}",
            stdout
        );

        Ok(())
    }
}
