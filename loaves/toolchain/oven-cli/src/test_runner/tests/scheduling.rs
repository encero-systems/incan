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
