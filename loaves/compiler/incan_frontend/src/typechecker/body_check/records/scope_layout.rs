//! Physical scope handles are allocated by the local checker. Records carry only their owner-relative tree.

use super::{Fields, NodeBindingError};
use crate::symbols::{ScopeKind, SymbolTable};

#[derive(Clone, Debug)]
pub(in crate::typechecker) struct CallableScopeLayout {
    root: usize,
    first: usize,
    children: Vec<(usize, ScopeKind)>,
}

impl CallableScopeLayout {
    pub(in crate::typechecker) fn capture(symbols: &SymbolTable, first: usize) -> Option<Self> {
        let root = symbols.current_scope_index();
        let mut layout = Self {
            root,
            first,
            children: Vec::new(),
        };
        for (parent, kind) in symbols.allocated_child_scopes(first) {
            if layout.children.len() == 2047 || !matches!(kind, ScopeKind::Block | ScopeKind::Function) {
                return None;
            }
            let ordinal = layout.ordinal(parent?)?;
            layout.children.push((ordinal, kind));
        }
        Some(layout)
    }

    fn ordinal(&self, scope: usize) -> Option<usize> {
        if scope == self.root {
            return Some(0);
        }
        let index = scope.checked_sub(self.first)?;
        (index < self.children.len()).then_some(index + 1)
    }

    fn absolute(&self, ordinal: usize) -> Option<usize> {
        if ordinal == 0 {
            return Some(self.root);
        }
        (ordinal <= self.children.len()).then(|| self.first + ordinal - 1)
    }

    pub(super) fn binding_ordinal(&self, scope: usize) -> Result<String, NodeBindingError> {
        self.ordinal(scope)
            .map(|ordinal| ordinal.to_string())
            .ok_or(NodeBindingError::Unsupported("lexical binding outside callable layout"))
    }

    pub(super) fn binding_scope(&self, ordinal: &str) -> Result<usize, NodeBindingError> {
        ordinal
            .parse::<usize>()
            .ok()
            .and_then(|ordinal| self.absolute(ordinal))
            .ok_or_else(|| NodeBindingError::InvalidField("undeclared callable scope".into()))
    }

    pub(super) fn row(&self) -> Vec<String> {
        let mut row = vec![
            "scope".into(),
            String::new(),
            "allocation".into(),
            self.children.len().to_string(),
        ];
        for (parent, kind) in &self.children {
            row.push(parent.to_string());
            row.push(
                match kind {
                    ScopeKind::Block => "block",
                    ScopeKind::Function => "function",
                    _ => unreachable!("checked scope kind"),
                }
                .to_string(),
            );
        }
        row
    }

    pub(super) fn decode(
        symbols: &SymbolTable,
        fields: &Fields,
        active: bool,
    ) -> Result<Option<Self>, NodeBindingError> {
        let mut rows = fields
            .iter()
            .filter(|row| row.first().is_some_and(|family| family == "scope"));
        let Some(row) = rows.next() else { return Ok(None) };
        let invalid = || NodeBindingError::InvalidField("callable scope allocation".into());
        if rows.next().is_some() || row.len() < 4 || row[1] != "" || row[2] != "allocation" {
            return Err(invalid());
        }
        let count = row[3].parse::<usize>().map_err(|_| invalid())?;
        if count > 2047 || row.len() != 4 + count * 2 {
            return Err(invalid());
        }
        let first = symbols.next_scope_index();
        let root = if active { symbols.current_scope_index() } else { first };
        let mut children = Vec::with_capacity(count);
        for pair in row[4..].chunks_exact(2) {
            let parent = pair[0].parse::<usize>().map_err(|_| invalid())?;
            if parent > children.len() {
                return Err(invalid());
            }
            let kind = match pair[1].as_str() {
                "block" => ScopeKind::Block,
                "function" => ScopeKind::Function,
                _ => return Err(invalid()),
            };
            children.push((parent, kind));
        }
        Ok(Some(Self {
            root,
            first: if active { first } else { first + 1 },
            children,
        }))
    }

    /// Commit only after every local/final row and current owner has passed its ordinary admission boundary.
    pub(super) fn install(&self, symbols: &mut SymbolTable) {
        debug_assert_eq!(self.root, symbols.current_scope_index());
        debug_assert_eq!(self.first, symbols.next_scope_index());
        for &(parent, kind) in &self.children {
            let physical = if parent == 0 {
                self.root
            } else {
                self.first + parent - 1
            };
            symbols.retain_exited_scope(physical, kind);
        }
    }
}
