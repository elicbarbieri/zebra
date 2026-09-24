use ::orchard::tree::MerkleHashOrchard;
use incrementalmerkletree::{Hashable, Level};

use crate::orchard::tests::vectors;
use crate::orchard::tree::*;

/// Upstream empty roots = the Orchard test vectors, leaf (level 0) to root
#[test]
fn empty_roots() {
    let _init_guard = zebra_test::init();

    for (level, expected) in vectors::EMPTY_ROOTS.iter().enumerate() {
        let level = u8::try_from(level).expect("33 levels");
        assert_eq!(
            MerkleHashOrchard::empty_root(Level::from(level)).to_bytes(),
            *expected,
            "level {level}"
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
            let cm_x = ::orchard::note::ExtractedNoteCommitment::from_bytes(cm_x_bytes).unwrap();

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
