/// Call the Incan provider's public facade, including its transitive library dependency.
pub fn native_price() -> i64 {
    native_cost::price()
}
