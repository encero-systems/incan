use super::*;

/// Compare the complete native representation as well as publication bytes. Derived Debug includes invocation-only
/// fields that serde intentionally skips, and its ordered containers retain every index, name and source coordinate.
/// The first differing encoded structural line is reported instead of hiding differences with normalization.
pub(super) fn assert_same_module(
    expected: &bir::BodyIrModule,
    actual: &bir::BodyIrModule,
) -> Result<(), Box<dyn std::error::Error>> {
    let before = format!("{expected:#?}");
    let after = format!("{actual:#?}");
    capture_module_baseline(expected, actual, &before, &after)?;
    if expected != actual || before.as_bytes() != after.as_bytes() {
        let mut left = before.lines();
        let mut right = after.lines();
        let mut line = 0;
        loop {
            let pair = (left.next(), right.next());
            if pair == (None, None) {
                return Err("native values differ despite equal Debug encodings".into());
            }
            line += 1;
            if pair.0 != pair.1 {
                return Err(format!("first structural difference at line {line}: {pair:?}").into());
            }
        }
    }
    assert_eq!(postcard::to_stdvec(expected)?, postcard::to_stdvec(actual)?);
    Ok(())
}
