//! Prepared Rust cases share immutable executables, while generated Incan batches retain source exclusion.

use std::path::{Path, PathBuf};

use super::{ActiveUnit, TestMarker, active_units_allow, build_execution_units, make_test};

/// Keep each Rust case independently schedulable so the worker limit is not defeated by pre-coalescing.
#[test]
fn prepared_rust_cases_remain_independent_units() {
    let tests = (0..8)
        .map(|i| {
            let mut test = make_test(&format!("native_{i}"), Vec::new());
            test.file_path = PathBuf::from("src/lib.rs");
            test
        })
        .collect::<Vec<_>>();
    for jobs in [1, 2, 3, 4] {
        let units = build_execution_units(&tests, Path::new("."), None, false, jobs);
        assert_eq!(units.len(), tests.len());
        assert!(units.iter().all(|unit| unit.tests.len() == 1));
    }
}

/// Worker coalescing cannot send Incan cases into Rust dispatch or Rust cases into an Incan harness.
#[test]
fn parallel_batches_preserve_language_boundaries() {
    let mut tests = (0..12)
        .map(|i| {
            let mut test = make_test(&format!("case_{i}"), Vec::new());
            test.file_path = if i % 2 == 0 {
                PathBuf::from("src/lib.rs")
            } else {
                PathBuf::from(format!("tests/test_{i}.incn"))
            };
            test
        })
        .collect::<Vec<_>>();
    for jobs in [1, 2, 3, 4] {
        for _ in 0..2 {
            let units = build_execution_units(&tests, Path::new("."), None, false, jobs);
            assert_eq!(units.iter().map(|unit| unit.tests.len()).sum::<usize>(), tests.len());
            for unit in units {
                let first = unit.tests[0].file_path.extension();
                assert!(unit.tests.iter().all(|test| test.file_path.extension() == first));
            }
            tests.reverse();
        }
    }
}

/// Prepared cases may share a root but still obey resource conflicts and exclusive serial scheduling.
#[test]
fn prepared_rust_cases_share_root_without_bypassing_constraints() -> Result<(), Box<dyn std::error::Error>> {
    let mut test = make_test("first", Vec::new());
    test.file_path = PathBuf::from("src/lib.rs");
    let mut other = test.clone();
    other.function_name = "second".into();
    let units = build_execution_units(&[test, other], Path::new("."), None, true, 2);
    let mut active = vec![ActiveUnit {
        index: 0,
        file_paths: units[0].file_paths.clone(),
        resources: units[0].resources.clone(),
        serial: false,
    }];
    let mut candidate = units.into_iter().nth(1).ok_or("expected a second execution unit")?;
    assert!(active_units_allow(&active, &candidate));
    active[0].resources.insert("db".into());
    candidate.resources.insert("db".into());
    assert!(!active_units_allow(&active, &candidate));
    candidate.resources.clear();
    active[0].serial = true;
    assert!(!active_units_allow(&active, &candidate));
    active[0].serial = false;
    candidate.serial = true;
    candidate.tests[0].markers.push(TestMarker::Serial);
    assert!(!active_units_allow(&active, &candidate));
    assert!(active_units_allow(&[], &candidate));
    Ok(())
}

/// A shared root consumes one exclusive global budget, while isolation preserves case-level scheduling.
#[test]
fn shared_native_groups_reserve_one_global_budget() -> Result<(), Box<dyn std::error::Error>> {
    let mut first = make_test("alpha", Vec::new());
    first.file_path = PathBuf::from("src/lib.rs");
    let mut second = first.clone();
    second.function_name = "beta".into();
    let mut third = first.clone();
    third.function_name = "other".into();
    third.file_path = PathBuf::from("tests/integration.rs");
    let tests = [first, second, third];
    let isolated = build_execution_units(&tests, Path::new("."), None, false, 18);
    assert_eq!(super::super::share_native_execution_units(isolated, 18, true).len(), 3);
    let units = build_execution_units(&tests, Path::new("."), None, false, 18);
    let shared = super::super::share_native_execution_units(units, 18, false);
    assert_eq!(shared.len(), 2);
    let root = shared.first().ok_or("missing root")?;
    assert_eq!(root.tests.len(), 2);
    assert_eq!(root.shared_threads, Some(18));
    assert!(root.serial);
    let active = [ActiveUnit {
        index: root.index,
        file_paths: root.file_paths.clone(),
        resources: root.resources.clone(),
        serial: root.serial,
    }];
    assert!(!active_units_allow(&active, shared.get(1).ok_or("missing other root")?));
    Ok(())
}

/// Time limits, resource conflicts, serial markers and Incan fixtures keep their existing execution boundaries.
#[test]
fn shared_native_groups_preserve_deadlines_and_incan_units() {
    let mut timed = make_test("timed", Vec::new());
    timed.file_path = PathBuf::from("src/lib.rs");
    timed.timeout = Some(std::time::Duration::from_secs(1));
    let mut serial = timed.clone();
    serial.function_name = "serial".into();
    serial.timeout = None;
    serial.markers.push(TestMarker::Serial);
    let mut resource = serial.clone();
    resource.function_name = "resource".into();
    resource.markers = vec![TestMarker::Resource("db".into())];
    let mut other_resource = resource.clone();
    other_resource.function_name = "other_resource".into();
    let incan = make_test("fixture_case", Vec::new());
    let units = build_execution_units(
        &[timed, serial, resource, other_resource, incan],
        Path::new("."),
        None,
        false,
        18,
    );
    let shared = super::super::share_native_execution_units(units, 18, false);
    assert_eq!(shared.len(), 5);
    assert!(shared.iter().all(|unit| unit.shared_threads.is_none()));
}
