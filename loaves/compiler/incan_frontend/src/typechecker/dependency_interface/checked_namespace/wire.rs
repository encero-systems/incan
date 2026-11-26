//! Typed native projection of the checker-owned namespace. Incan owns the Crumb descriptions and row codec.

use std::collections::{BTreeMap, BTreeSet};

use incan_semantics_core::body_ir::SharedIdentities;

/// Logical cells passed across the checked frontend to graph-adapter boundary. A field selector is a six-byte
/// Crumb member identity, represented as a u64 only while the native adapter carries the logical value.
#[derive(Clone, Debug, PartialEq)]
pub enum NamespaceCell {
    Text(String),
    Binary(Vec<u8>),
    Boolean(bool),
    Signed(i64),
    Unsigned(u64),
    /// The complete signed payload of a checked integer or decimal coefficient.
    WideSigned(i128),
    /// The complete payload of a checked unsigned integer.
    WideUnsigned(u128),
    Float(f64),
    None,
    Some(Box<Self>),
    List(Vec<Self>),
    Tuple(Vec<Self>),
    Map(Vec<(Self, Self)>),
    Record(Vec<(u64, Self)>),
    Variant(u64, Vec<(u64, Self)>),
}

/// Codec supplied by the native driver, whose dependency on the Incan compiler-record Loaf owns actual wire I/O.
pub trait NamespaceCrumb {
    fn write(&self, root: NamespaceCell) -> Result<Vec<u8>, String>;
    fn read(&self, encoded: &[u8]) -> Result<NamespaceCell, String>;
}

type Cell = NamespaceCell;
pub(crate) trait Project: Sized {
    fn project(&self) -> Cell;
    fn restore(value: Cell) -> Result<Self, String>;
    fn project_list(values: &[Self]) -> Cell {
        Cell::List(values.iter().map(Self::project).collect())
    }
    fn restore_list(value: Cell) -> Result<Vec<Self>, String> {
        let Cell::List(items) = value else {
            return Err("expected namespace list".into());
        };
        items.into_iter().map(Self::restore).collect()
    }
}

macro_rules! scalar {
    ($ty:ty, $tag:ident) => {
        impl Project for $ty {
            fn project(&self) -> Cell {
                Cell::$tag(self.clone())
            }
            fn restore(value: Cell) -> Result<Self, String> {
                match value {
                    Cell::$tag(value) => Ok(value),
                    _ => Err(concat!("expected namespace ", stringify!($ty)).into()),
                }
            }
        }
    };
}
scalar!(String, Text);
scalar!(bool, Boolean);
scalar!(i64, Signed);
scalar!(u64, Unsigned);
scalar!(f64, Float);
impl Project for i128 {
    /// Carry the exact checked bits into Crumb's signed logical integer domain.
    fn project(&self) -> Cell {
        Cell::WideSigned(*self)
    }

    /// Restore the same value whether the wire reader could hold it in a narrow cell or needed all 128 bits.
    fn restore(value: Cell) -> Result<Self, String> {
        match value {
            Cell::Signed(number) => Ok(number.into()),
            Cell::WideSigned(number) => Ok(number),
            _ => Err("expected checked signed integer".into()),
        }
    }
}
impl Project for u128 {
    /// Carry the exact checked bits into Crumb's unsigned logical integer domain.
    fn project(&self) -> Cell {
        Cell::WideUnsigned(*self)
    }

    /// Restore an unsigned integer in its complete domain.
    fn restore(value: Cell) -> Result<Self, String> {
        match value {
            Cell::Unsigned(number) => Ok(number.into()),
            Cell::WideUnsigned(number) => Ok(number),
            _ => Err("expected checked unsigned integer".into()),
        }
    }
}
macro_rules! unsigned {
    ($ty:ty) => {
        impl Project for $ty {
            fn project(&self) -> Cell {
                Cell::Unsigned(*self as u64)
            }
            fn restore(value: Cell) -> Result<Self, String> {
                u64::restore(value)?
                    .try_into()
                    .map_err(|_| concat!("namespace integer exceeds ", stringify!($ty)).into())
            }
        }
    };
}
unsigned!(u32);
unsigned!(usize);
impl Project for u8 {
    fn project(&self) -> Cell {
        Cell::Unsigned(u64::from(*self))
    }
    fn restore(value: Cell) -> Result<Self, String> {
        u64::restore(value)?
            .try_into()
            .map_err(|_| "namespace integer exceeds u8".into())
    }
    fn project_list(values: &[Self]) -> Cell {
        Cell::Binary(values.to_vec())
    }
    fn restore_list(value: Cell) -> Result<Vec<Self>, String> {
        match value {
            Cell::Binary(bytes) => Ok(bytes),
            _ => Err("expected namespace bytes".into()),
        }
    }
}
impl Project for [u8; 32] {
    fn project(&self) -> Cell {
        Cell::Binary(self.to_vec())
    }
    fn restore(value: Cell) -> Result<Self, String> {
        u8::restore_list(value)?
            .try_into()
            .map_err(|_| "namespace expansion identity needs 32 bytes".into())
    }
}
impl<T: Project> Project for Box<T> {
    fn project(&self) -> Cell {
        (**self).project()
    }
    fn restore(value: Cell) -> Result<Self, String> {
        T::restore(value).map(Box::new)
    }
}
impl<T: Project> Project for Vec<T> {
    fn project(&self) -> Cell {
        T::project_list(self)
    }
    fn restore(value: Cell) -> Result<Self, String> {
        T::restore_list(value)
    }
}
impl<T: Project> Project for Option<T> {
    fn project(&self) -> Cell {
        self.as_ref()
            .map_or(Cell::None, |value| Cell::Some(Box::new(value.project())))
    }
    fn restore(value: Cell) -> Result<Self, String> {
        match value {
            Cell::None => Ok(None),
            Cell::Some(v) => T::restore(*v).map(Some),
            _ => Err("expected namespace option".into()),
        }
    }
}
/// Shared native tables use their ordinary typed field representation on the wire.
impl<T: Project> Project for std::sync::Arc<T> {
    /// Project the single shared value without copying its native table.
    fn project(&self) -> Cell {
        self.as_ref().project()
    }

    /// Allocate the checked table once for the namespace and its installed dependency interfaces.
    fn restore(value: Cell) -> Result<Self, String> {
        T::restore(value).map(std::sync::Arc::new)
    }
}
impl<A: Project, B: Project> Project for (A, B) {
    fn project(&self) -> Cell {
        Cell::Tuple(vec![self.0.project(), self.1.project()])
    }
    fn restore(value: Cell) -> Result<Self, String> {
        let Cell::Tuple(items) = value else {
            return Err("expected namespace pair".into());
        };
        let [a, b] = items.try_into().map_err(|_| "namespace pair needs two values")?;
        Ok((A::restore(a)?, B::restore(b)?))
    }
}
impl<T: Project + Ord> Project for BTreeSet<T> {
    fn project(&self) -> Cell {
        Cell::List(self.iter().map(T::project).collect())
    }
    fn restore(value: Cell) -> Result<Self, String> {
        let items = T::restore_list(value)?;
        if !items.windows(2).all(|pair| pair[0] < pair[1]) {
            return Err("namespace set is not strictly ordered".into());
        }
        Ok(items.into_iter().collect())
    }
}
impl<K: Project + Ord, T: Project> Project for BTreeMap<K, T> {
    /// Project each key and its binding without substituting a text rendering of the key.
    fn project(&self) -> Cell {
        Cell::Map(
            self.iter()
                .map(|(key, value)| (key.project(), value.project()))
                .collect(),
        )
    }
    /// Restore the typed map and refuse duplicate keys in their semantic domain.
    fn restore(value: Cell) -> Result<Self, String> {
        let Cell::Map(entries) = value else {
            return Err("expected namespace map".into());
        };
        let mut result = Self::new();
        for (key, value) in entries {
            let key = K::restore(key)?;
            if result.insert(key, T::restore(value)?).is_some() {
                return Err("duplicate namespace map key".into());
            }
        }
        Ok(result)
    }
}

fn read_fields<const N: usize>(fields: Vec<(u64, Cell)>, expected: [u64; N]) -> Result<[Cell; N], String> {
    let mut fields: [(u64, Cell); N] = fields.try_into().map_err(|_| "incorrect namespace field count")?;
    for (index, id) in expected.into_iter().enumerate() {
        if fields[index].0 != id {
            let remaining = index + 1;
            let offset = fields[remaining..]
                .iter()
                .position(|(candidate, _)| *candidate == id)
                .ok_or("incorrect namespace field identity")?;
            fields.swap(index, remaining + offset);
        }
    }
    Ok(fields.map(|(_, value)| value))
}

macro_rules! model { ($ty:ty { $($f:ident : $id:literal),* $(,)? }) => {impl Project for $ty {
    fn project(&self) -> Cell { let Self{$($f),*}=self; Cell::Record(vec![$(($id,$f.project())),*]) }
    fn restore(value: Cell) -> Result<Self,String> { let Cell::Record(fields)=value else{return Err(concat!("expected namespace model ",stringify!($ty)).into())}; let [$($f),*]=read_fields(fields,[$($id),*])?; Ok(Self{$($f:Project::restore($f)?),*}) }
}}; }
macro_rules! sum { ($ty:ty { $($v:ident = $tag:literal $( ( $($p:ident : $pid:literal),* ) )? $( { $($f:ident : $fid:literal),* } )? ),* $(,)? }) => {impl Project for $ty {
    fn project(&self)->Cell { match self {$(Self::$v $( ( $($p),* ) )? $( { $($f),* } )? => Cell::Variant($tag,vec![$($(( $pid, $p.project() )),*)? $($(( $fid, $f.project() )),*)?]),)*} }
    fn restore(value:Cell)->Result<Self,String>{let Cell::Variant(tag,fields)=value else{return Err(concat!("expected namespace sum ",stringify!($ty)).into())};match tag { $($tag=>{let [$($($p),*)? $($($f),*)?]=read_fields(fields,[$($($pid),*)? $($($fid),*)?])?;Ok(Self::$v $( ( $(Project::restore($p)?),* ) )? $( { $($f:Project::restore($f)?),* } )?)},)* _=>Err(concat!("unknown namespace alternative for ",stringify!($ty)).into())} }
}}; }

mod atoms;
pub(crate) mod callable_fields;
mod declarations;
mod executable;
pub mod executable_frames;
pub use atoms::{to_atom, to_cell};
#[cfg(test)]
pub(super) use executable::verify_executable_codec;
pub use executable::{
    executable_body_value, executable_module_value, restore_executable_body_value, restore_executable_module_value,
};

pub(super) fn project(value: &super::CheckedNamespaceInterface) -> NamespaceCell {
    value.project()
}
pub(super) fn restore(value: NamespaceCell) -> Result<super::CheckedNamespaceInterface, String> {
    super::CheckedNamespaceInterface::restore(value)
}

/// Project the complete checked nominal scope, including positioned identities, for the graph scope writer.
pub fn executable_scope_value(scope: &SharedIdentities) -> NamespaceCell {
    scope.project()
}

/// Restore one authenticated nominal map as a single shared handle for its executable owners.
pub fn restore_executable_scope_value(value: NamespaceCell) -> Result<SharedIdentities, String> {
    SharedIdentities::restore(value)
}
