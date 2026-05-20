use native_consumer_core::{add_count, make_counter, rename_counter, Counter};

#[test]
fn calls_generated_public_model_and_functions() {
    let made = make_counter("rust-host".to_string(), 41);
    assert_eq!(made.label, "rust-host");
    assert_eq!(made.count, 41);

    let direct_for_bump = Counter {
        label: "manual".to_string(),
        count: 2,
    };
    let bumped = add_count(direct_for_bump, 5);
    assert_eq!(bumped.label, "manual");
    assert_eq!(bumped.count, 7);

    let direct_for_rename = Counter {
        label: "manual".to_string(),
        count: 2,
    };
    let renamed = rename_counter(direct_for_rename, "renamed".to_string());
    assert_eq!(renamed.label, "renamed");
    assert_eq!(renamed.count, 2);
}
