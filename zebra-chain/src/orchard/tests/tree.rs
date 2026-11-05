use halo2::pasta::{group::ff::PrimeField, pallas};

use crate::orchard::tests::vectors;
use crate::orchard::tree::*;
use crate::subtree::{NoteCommitmentSubtreeIndex, TRACKED_SUBTREE_HEIGHT};

#[test]
fn empty_roots() {
    let _init_guard = zebra_test::init();

    for i in 0..EMPTY_ROOTS.len() {
        assert_eq!(
            EMPTY_ROOTS[i].to_repr(),
            // The test vector is in reversed order.
            vectors::EMPTY_ROOTS[usize::from(MERKLE_DEPTH) - i]
        );
    }
}

#[test]
fn incremental_roots() {
    let _init_guard = zebra_test::init();

    let mut leaves = vec![];

    let mut incremental_tree = NoteCommitmentTree::default();

    for (i, commitment_set) in vectors::COMMITMENTS.iter().enumerate() {
        for cm_x_bytes in commitment_set.iter() {
            let cm_x = pallas::Base::from_repr(*cm_x_bytes).unwrap();

            leaves.push(cm_x);

            let _ = incremental_tree.append(cm_x);
        }

        assert_eq!(
            hex::encode(incremental_tree.hash()),
            hex::encode(vectors::ROOTS[i].anchor)
        );

        assert_eq!(
            hex::encode((NoteCommitmentTree::from(leaves.clone())).hash()),
            hex::encode(vectors::ROOTS[i].anchor)
        );
    }
}

/// Per-append updater as the block path ran it before `append_batch`
fn append_each(
    tree: &mut NoteCommitmentTree,
    cm_xs: &[pallas::Base],
) -> Option<(NoteCommitmentSubtreeIndex, Node)> {
    let mut completed = None;
    for &cm_x in cm_xs {
        tree.append(cm_x).expect("below capacity");
        if let Some(index_and_root) = tree.completed_subtree_index_and_root() {
            completed = Some(index_and_root);
        }
    }
    completed
}

fn leaves(seed: u64, n: usize) -> Vec<pallas::Base> {
    use halo2::pasta::group::ff::Field;
    use rand::SeedableRng;
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(seed);
    (0..n).map(|_| pallas::Base::random(&mut rng)).collect()
}

#[test]
fn append_batch_reports_the_subtree_the_block_completes() {
    let _init_guard = zebra_test::init();
    let subtree = 1usize << TRACKED_SUBTREE_HEIGHT;

    let mut each = NoteCommitmentTree::default();
    append_each(&mut each, &leaves(1, subtree - 3));
    let mut batch = each.clone();

    // empty block: no change, no subtree
    assert_eq!(batch.append_batch(vec![]).unwrap(), None);
    batch.assert_frontier_eq(&each);

    // block completes subtree 0 mid-batch (3 leaves in, 2 past it)
    let block = leaves(2, 5);
    let expected = append_each(&mut each, &block);
    assert_eq!(expected.map(|(index, _)| index.0), Some(0));
    assert_eq!(batch.append_batch(block).unwrap(), expected);
    assert_eq!(batch.root(), each.root());
    batch.assert_frontier_eq(&each);

    // block ends inside subtree 1: nothing completed
    let block = leaves(3, 7);
    assert_eq!(append_each(&mut each, &block), None);
    assert_eq!(batch.append_batch(block).unwrap(), None);
    batch.assert_frontier_eq(&each);

    // block's last leaf completes subtree 1 exactly (tip path, not the level loop)
    let block = leaves(4, subtree - 9);
    let expected = append_each(&mut each, &block);
    assert_eq!(expected.map(|(index, _)| index.0), Some(1));
    assert_eq!(batch.append_batch(block).unwrap(), expected);
    assert_eq!(batch.position(), Some(2 * subtree as u64 - 1));
    assert_eq!(batch.root(), each.root());
    batch.assert_frontier_eq(&each);
}

#[test]
fn combine_pairs_matches_combine_on_full_width_children() {
    use incrementalmerkletree::Hashable;

    let mut values: Vec<pallas::Base> = [0u64, 1, 2, 7, 65_535, u64::MAX]
        .into_iter()
        .map(pallas::Base::from)
        .collect();
    values.extend([
        -pallas::Base::one(),
        -pallas::Base::from(2),
        pallas::Base::from_raw([u64::MAX; 4]),
        pallas::Base::from_raw([0, 0, 0, u64::MAX]),
        pallas::Base::from_raw([
            0x0123_4567_89ab_cdef,
            0xfedc_ba98_7654_3210,
            0xdead_beef_cafe_babe,
            0x0f1e_2d3c_4b5a_6978,
        ]),
    ]);
    let nodes: Vec<Node> = values.into_iter().map(Node::from).collect();
    let children: Vec<Node> = nodes
        .iter()
        .flat_map(|left| nodes.iter().map(move |right| [*left, *right]))
        .flatten()
        .collect();

    for level in 0..MERKLE_DEPTH {
        let level = incrementalmerkletree::Level::from(level);
        let expected: Vec<Node> = children
            .as_chunks::<2>()
            .0
            .iter()
            .map(|[left, right]| Node::combine(level, left, right))
            .collect();
        assert_eq!(
            Node::combine_pairs(level, &children),
            expected,
            "level {level:?}"
        );
    }
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(48))]
    #[test]
    fn append_batch_matches_per_append_across_blocks(
        start in 0usize..600,
        widths in proptest::collection::vec(0usize..160, 1..4),
    ) {
        let mut each = NoteCommitmentTree::default();
        append_each(&mut each, &leaves(start as u64, start));
        let mut batch = each.clone();

        for (i, width) in widths.into_iter().enumerate() {
            let block = leaves(1000 + i as u64, width);
            let expected = append_each(&mut each, &block);
            proptest::prop_assert_eq!(batch.append_batch(block).unwrap(), expected);
            proptest::prop_assert_eq!(batch.root(), each.root());
            proptest::prop_assert_eq!(batch.position(), each.position());
            proptest::prop_assert_eq!(batch.to_rpc_bytes(), each.to_rpc_bytes());
        }
    }
}
