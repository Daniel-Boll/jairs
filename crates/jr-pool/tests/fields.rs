//! Visible-field lookup through direct and `using`-promoted fields.

use jr_base::{FileId, Interner};
use jr_pool::{DeclId, Field, FieldLookup, FieldStep, Pool, PoolId};

fn decl(index: u32) -> DeclId {
    DeclId::new(FileId::from_u32(0), index)
}

#[test]
fn direct_lookup_uses_concrete_reordered_index_and_auto_dereferences_root() {
    let interner = Interner::new();
    let mut pool = Pool::new();
    let point_decl = decl(0);
    let point = pool.struct_type(point_decl);
    let x = interner.intern("x");
    let y = interner.intern("y");
    let z = interner.intern("z");
    pool.set_struct_fields(
        point_decl,
        vec![
            Field::new(z, PoolId::BOOL),
            Field::new(x, PoolId::S64),
            Field::new(y, PoolId::U8),
        ],
    );

    assert_eq!(
        pool.visible_field(point, x),
        FieldLookup::Unique {
            ty: PoolId::S64,
            path: vec![FieldStep::Field(1)],
        }
    );

    let point_pointer = pool.pointer_to(point);
    assert_eq!(
        pool.visible_field(point_pointer, y),
        FieldLookup::Unique {
            ty: PoolId::U8,
            path: vec![FieldStep::Deref, FieldStep::Field(2)],
        }
    );
}

#[test]
fn nested_using_returns_every_concrete_field_step() {
    let interner = Interner::new();
    let mut pool = Pool::new();
    let leaf_decl = decl(0);
    let middle_decl = decl(1);
    let outer_decl = decl(2);
    let leaf = pool.struct_type(leaf_decl);
    let middle = pool.struct_type(middle_decl);
    let outer = pool.struct_type(outer_decl);
    let x = interner.intern("x");

    pool.set_struct_fields(leaf_decl, vec![Field::new(x, PoolId::S64)]);
    pool.set_struct_fields(
        middle_decl,
        vec![
            Field::new(interner.intern("padding"), PoolId::U8),
            Field::embedded(interner.intern("leaf"), leaf),
        ],
    );
    pool.set_struct_fields(
        outer_decl,
        vec![
            Field::new(interner.intern("tag"), PoolId::BOOL),
            Field::embedded(interner.intern("middle"), middle),
        ],
    );

    assert_eq!(
        pool.visible_field(outer, x),
        FieldLookup::Unique {
            ty: PoolId::S64,
            path: vec![
                FieldStep::Field(1),
                FieldStep::Field(1),
                FieldStep::Field(0),
            ],
        }
    );
}

#[test]
fn pointer_valued_using_emits_a_dereference() {
    let interner = Interner::new();
    let mut pool = Pool::new();
    let inner_decl = decl(0);
    let outer_decl = decl(1);
    let inner = pool.struct_type(inner_decl);
    let outer = pool.struct_type(outer_decl);
    let x = interner.intern("x");
    pool.set_struct_fields(inner_decl, vec![Field::new(x, PoolId::S64)]);

    let inner_pointer = pool.pointer_to(inner);
    pool.set_struct_fields(
        outer_decl,
        vec![Field::embedded(interner.intern("inner"), inner_pointer)],
    );

    assert_eq!(
        pool.visible_field(outer, x),
        FieldLookup::Unique {
            ty: PoolId::S64,
            path: vec![FieldStep::Field(0), FieldStep::Deref, FieldStep::Field(0),],
        }
    );
}

#[test]
fn nearest_using_depth_shadows_deeper_matches() {
    let interner = Interner::new();
    let mut pool = Pool::new();
    let near_decl = decl(0);
    let deep_decl = decl(1);
    let far_decl = decl(2);
    let root_decl = decl(3);
    let near = pool.struct_type(near_decl);
    let deep = pool.struct_type(deep_decl);
    let far = pool.struct_type(far_decl);
    let root = pool.struct_type(root_decl);
    let x = interner.intern("x");

    pool.set_struct_fields(near_decl, vec![Field::new(x, PoolId::U8)]);
    pool.set_struct_fields(deep_decl, vec![Field::new(x, PoolId::S64)]);
    pool.set_struct_fields(
        far_decl,
        vec![Field::embedded(interner.intern("deep"), deep)],
    );
    pool.set_struct_fields(
        root_decl,
        vec![
            Field::embedded(interner.intern("near"), near),
            Field::embedded(interner.intern("far"), far),
        ],
    );

    assert_eq!(
        pool.visible_field(root, x),
        FieldLookup::Unique {
            ty: PoolId::U8,
            path: vec![FieldStep::Field(0), FieldStep::Field(0)],
        }
    );
}

#[test]
fn two_paths_to_the_same_type_are_ambiguous_at_the_winning_depth() {
    let interner = Interner::new();
    let mut pool = Pool::new();
    let shared_decl = decl(0);
    let left_decl = decl(1);
    let right_decl = decl(2);
    let root_decl = decl(3);
    let shared = pool.struct_type(shared_decl);
    let left = pool.struct_type(left_decl);
    let right = pool.struct_type(right_decl);
    let root = pool.struct_type(root_decl);
    let x = interner.intern("x");

    pool.set_struct_fields(shared_decl, vec![Field::new(x, PoolId::S64)]);
    pool.set_struct_fields(
        left_decl,
        vec![Field::embedded(interner.intern("shared_left"), shared)],
    );
    pool.set_struct_fields(
        right_decl,
        vec![Field::embedded(interner.intern("shared_right"), shared)],
    );
    pool.set_struct_fields(
        root_decl,
        vec![
            Field::embedded(interner.intern("left"), left),
            Field::embedded(interner.intern("right"), right),
        ],
    );

    assert_eq!(
        pool.visible_field(root, x),
        FieldLookup::Ambiguous {
            paths: vec![
                vec![
                    FieldStep::Field(0),
                    FieldStep::Field(0),
                    FieldStep::Field(0),
                ],
                vec![
                    FieldStep::Field(1),
                    FieldStep::Field(0),
                    FieldStep::Field(0),
                ],
            ],
        }
    );
}

#[test]
fn recursive_using_cycle_terminates_when_the_field_is_missing() {
    let interner = Interner::new();
    let mut pool = Pool::new();
    let node_decl = decl(0);
    let node = pool.struct_type(node_decl);
    let node_pointer = pool.pointer_to(node);
    pool.set_struct_fields(
        node_decl,
        vec![Field::embedded(interner.intern("next"), node_pointer)],
    );

    assert_eq!(
        pool.visible_field(node, interner.intern("missing")),
        FieldLookup::Missing
    );
}

#[test]
fn parameterized_using_reads_substituted_instance_fields() {
    let interner = Interner::new();
    let mut pool = Pool::new();
    let box_decl = decl(0);
    let wrapper_decl = decl(1);
    let box_instance = pool.struct_instance(box_decl, vec![PoolId::S64]);
    let wrapper = pool.struct_type(wrapper_decl);
    let value = interner.intern("value");

    pool.set_struct_fields(box_decl, vec![Field::new(value, PoolId::ERROR)]);
    pool.set_instance_fields(box_instance, vec![Field::new(value, PoolId::S64)]);
    pool.set_struct_fields(
        wrapper_decl,
        vec![Field::embedded(interner.intern("box"), box_instance)],
    );

    assert_eq!(
        pool.visible_field(wrapper, value),
        FieldLookup::Unique {
            ty: PoolId::S64,
            path: vec![FieldStep::Field(0), FieldStep::Field(0)],
        }
    );
}
