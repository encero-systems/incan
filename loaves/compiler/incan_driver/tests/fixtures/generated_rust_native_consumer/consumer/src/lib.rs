#[test]
/// Verify native Rust can retain public construction, factories, and projections.
fn calls_generated_public_model_and_functions() {
    use native_consumer_core::{
        add_count, make_all_public, make_counter, rename_counter, Admission, AllPublic, Counter, Defaulted, Mixed,
        PrivateTypeModel,
    };

    let direct = AllPublic { value: 17 };
    assert_eq!(direct.value, 17);
    assert_eq!(make_all_public(18).value, 18);

    let admitted = Admission::admitted();
    assert!(admitted.allowed());

    let mixed = Mixed::sample();
    assert_eq!(mixed.label, "mixed");
    assert!(mixed.allowed());

    let private_type = PrivateTypeModel::locked();
    assert_eq!(private_type.code(), 7);

    let defaulted = Defaulted("native".to_string());
    assert_eq!(defaulted.label, "native");
    assert!(defaulted.allowed());

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
