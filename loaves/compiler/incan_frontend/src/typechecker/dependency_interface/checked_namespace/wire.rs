//! Typed native projection of the checker-owned namespace. Incan owns the Crumb descriptions and row codec.

use std::collections::{BTreeMap, BTreeSet};

/// Logical cells passed across the checked frontend to graph-adapter boundary. A field selector is a six-byte
/// Crumb member identity, represented as a u64 only while the native adapter carries the logical value.
#[derive(Clone, Debug, PartialEq)]
pub enum NamespaceCell {
    Text(String),
    Binary(Vec<u8>),
    Boolean(bool),
    Signed(i64),
    Unsigned(u64),
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
trait Project: Sized {
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
impl<T: Project> Project for BTreeMap<String, T> {
    fn project(&self) -> Cell {
        Cell::Map(
            self.iter()
                .map(|(key, value)| (key.project(), value.project()))
                .collect(),
        )
    }
    fn restore(value: Cell) -> Result<Self, String> {
        let Cell::Map(entries) = value else {
            return Err("expected namespace map".into());
        };
        let mut result = Self::new();
        for (key, value) in entries {
            let key = String::restore(key)?;
            if result.insert(key, T::restore(value)?).is_some() {
                return Err("duplicate namespace map key".into());
            }
        }
        Ok(result)
    }
}

fn read_fields<const N: usize>(fields: Vec<(u64, Cell)>, expected: [u64; N]) -> Result<[Cell; N], String> {
    if fields.len() != N {
        return Err("incorrect namespace field count".into());
    }
    let mut source: BTreeMap<_, _> = fields.into_iter().collect();
    let ordered = expected
        .into_iter()
        .map(|id| {
            source
                .remove(&id)
                .ok_or_else(|| "incorrect namespace field identity".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    ordered.try_into().map_err(|_| "incorrect namespace field order".into())
}

macro_rules! model { ($ty:ty { $($f:ident : $id:literal),* $(,)? }) => {impl Project for $ty {
    fn project(&self) -> Cell { let Self{$($f),*}=self; Cell::Record(vec![$(($id,$f.project())),*]) }
    fn restore(value: Cell) -> Result<Self,String> { let Cell::Record(fields)=value else{return Err(concat!("expected namespace model ",stringify!($ty)).into())}; let [$($f),*]=read_fields(fields,[$($id),*])?; Ok(Self{$($f:Project::restore($f)?),*}) }
}}; }
macro_rules! sum { ($ty:ty { $($v:ident = $tag:literal $( ( $($p:ident : $pid:literal),* ) )? $( { $($f:ident : $fid:literal),* } )? ),* $(,)? }) => {impl Project for $ty {
    fn project(&self)->Cell { match self {$(Self::$v $( ( $($p),* ) )? $( { $($f),* } )? => Cell::Variant($tag,vec![$($(( $pid, $p.project() )),*)? $($(( $fid, $f.project() )),*)?]),)*} }
    fn restore(value:Cell)->Result<Self,String>{let Cell::Variant(tag,fields)=value else{return Err(concat!("expected namespace sum ",stringify!($ty)).into())};match tag { $($tag=>{let [$($($p),*)? $($($f),*)?]=read_fields(fields,[$($($pid),*)? $($($fid),*)?])?;Ok(Self::$v $( ( $(Project::restore($p)?),* ) )? $( { $($f:Project::restore($f)?),* } )?)},)* _=>Err(concat!("unknown namespace alternative for ",stringify!($ty)).into())} }
}}; }

mod declarations;
pub(super) fn project(value: &super::CheckedNamespaceInterface) -> NamespaceCell {
    value.project()
}
pub(super) fn restore(value: NamespaceCell) -> Result<super::CheckedNamespaceInterface, String> {
    super::CheckedNamespaceInterface::restore(value)
}
