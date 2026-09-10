//! Visible-field resolution through `using` embeddings (ADR-0233 §3).

use jr_base::Symbol;

use crate::{Item, Pool, PoolId};

/// One concrete projection step from a receiver to a visible field.
///
/// The path is deliberately executable by MIR: it contains every implicit
/// pointer dereference and every concrete field index selected while following
/// `using` embeddings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldStep {
    /// Dereference the current pointer value.
    Deref,
    /// Project the field at this declaration-order index.
    Field(u32),
}

/// The result of resolving a field name through direct and `using` fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldLookup {
    /// The receiver exposes no field with this name.
    Missing,
    /// Exactly one nearest field is visible.
    Unique {
        /// The resolved type of the final field.
        ty: PoolId,
        /// Concrete projection steps from the original receiver.
        path: Vec<FieldStep>,
    },
    /// More than one field is visible at the nearest promotion depth.
    Ambiguous {
        /// Every concrete path found at that depth, in declaration order.
        paths: Vec<Vec<FieldStep>>,
    },
}

#[derive(Debug)]
struct SearchNode {
    ty: PoolId,
    path: Vec<FieldStep>,
    /// Effective aggregate types visited along this one promotion path.
    ///
    /// This is per node rather than global: two independent paths reaching the
    /// same embedded type are an ambiguity, not duplicate work to discard.
    visited: Vec<PoolId>,
}

impl Pool {
    /// Resolves `name` as a direct or `using`-promoted field of `root`.
    ///
    /// The root and each `using` field are auto-dereferenced. A direct field
    /// wins; otherwise promotion is breadth-first, so a unique field at the
    /// nearest depth wins and multiple fields at that depth are ambiguous.
    /// Recursive embedding is cycle-safe per search path.
    #[must_use]
    pub fn visible_field(&self, root: PoolId, name: Symbol) -> FieldLookup {
        let mut root_path = Vec::new();
        let root = self.auto_deref(root, &mut root_path);
        let mut frontier = vec![SearchNode {
            ty: root,
            path: root_path,
            visited: vec![root],
        }];

        loop {
            let mut matches = Vec::new();
            for node in &frontier {
                let Some(fields) = self.fields_of(node.ty) else {
                    continue;
                };
                for (index, field) in fields.iter().enumerate() {
                    if field.name == name {
                        let mut path = node.path.clone();
                        path.push(FieldStep::Field(field_index(index)));
                        matches.push((field.ty, path));
                    }
                }
            }

            match matches.len() {
                0 => {}
                1 => {
                    let (ty, path) = matches.pop().expect("one match was counted");
                    return FieldLookup::Unique { ty, path };
                }
                _ => {
                    return FieldLookup::Ambiguous {
                        paths: matches.into_iter().map(|(_, path)| path).collect(),
                    };
                }
            }

            let mut next = Vec::new();
            for node in frontier {
                let Some(fields) = self.fields_of(node.ty) else {
                    continue;
                };
                for (index, field) in fields.iter().enumerate() {
                    if !field.using {
                        continue;
                    }

                    let mut path = node.path.clone();
                    path.push(FieldStep::Field(field_index(index)));
                    let embedded = self.auto_deref(field.ty, &mut path);
                    if node.visited.contains(&embedded) || self.fields_of(embedded).is_none() {
                        continue;
                    }

                    let mut visited = node.visited.clone();
                    visited.push(embedded);
                    next.push(SearchNode {
                        ty: embedded,
                        path,
                        visited,
                    });
                }
            }

            if next.is_empty() {
                return FieldLookup::Missing;
            }
            frontier = next;
        }
    }

    fn auto_deref(&self, mut ty: PoolId, path: &mut Vec<FieldStep>) -> PoolId {
        while let Item::PointerType(pointee) = self.item(ty) {
            path.push(FieldStep::Deref);
            ty = *pointee;
        }
        ty
    }
}

fn field_index(index: usize) -> u32 {
    u32::try_from(index).expect("a field index must fit in u32")
}
