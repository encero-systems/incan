//! Shared source inputs for exact numeric default and static-result controls (#1337).

pub(super) const DEFAULTS: &str = r#"@derive(Default)
model ExactDefaults:
    signed8: i8
    signed16: i16
    signed32: i32
    signed128: i128
    signed_size: isize
    unsigned8: u8
    unsigned16: u16
    unsigned32: u32
    unsigned64: u64
    unsigned128: u128
    unsigned_size: usize
    single: f32
    double: f64
    ordinary_int: int
    ordinary_float: float
    chosen_byte: u8 = 7
    chosen_single: f32 = -1.0

@derive(Default)
model NestedDefaults:
    inner: ExactDefaults

def main() -> None:
    value = NestedDefaults.default().inner
    println(value.signed8)
    println(value.signed16)
    println(value.signed32)
    println(value.signed128)
    println(value.signed_size)
    println(value.unsigned8)
    println(value.unsigned16)
    println(value.unsigned32)
    println(value.unsigned64)
    println(value.unsigned128)
    println(value.unsigned_size)
    println(value.single)
    println(value.double)
    println(value.ordinary_int)
    println(value.ordinary_float)
    println(value.chosen_byte)
    println(value.chosen_single)
"#;

pub(super) const STATIC_RESULTS: &str = r#"model NumericCell:
    single: f32
    signed: i32
    byte: u8

    def read_single(self) -> f32:
        return self.single

    def read_signed(self) -> i32:
        return self.signed

    def read_byte(self) -> u8:
        return self.byte

static NUMBERS: NumericCell = NumericCell(single=1.5, signed=-7, byte=255)

def main() -> None:
    println(NUMBERS.read_single())
    println(NUMBERS.read_signed())
    println(NUMBERS.read_byte())
"#;
