use typed_boundary::caller::incan::{Op, Plan, make_plan};

fn main() {
    let returned = make_plan();
    println!("returned: label={} version={}", returned.label, returned.version);
    for op in &returned.ops {
        match op {
            Op::Assign(dst, value) => println!("returned: assign dst={dst} value={value}"),
            Op::Return => println!("returned: return"),
        }
    }
    let constructed = Plan {
        ops: vec![Op::Assign(9, 81), Op::Return],
        label: "rust-built".to_string(),
        version: 4,
    };
    println!("constructed: label={} version={}", constructed.label, constructed.version);
    drop(returned);
    drop(constructed);
    println!("drops completed");
}
