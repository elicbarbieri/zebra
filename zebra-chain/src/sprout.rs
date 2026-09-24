//! Sprout-related functionality.
//!
//! - JoinSplits = `zcash_primitives` sprout bundle (one JoinSplit codec)
//! - Nullifiers, note commitments, tree = Zebra's (state keys, no upstream Sprout tree)

#[cfg(any(test, feature = "proptest-impl"))]
pub mod arbitrary;
#[cfg(test)]
mod tests;

pub mod commitment;
pub mod note;
pub mod tree;

pub use commitment::NoteCommitment;
pub use note::Nullifier;

/// JoinSplit section of a v2-v4 transaction
pub type JoinSplitData = zcash_primitives::transaction::components::sprout::Bundle;

/// JoinSplit description (BCTV14 proof in v2/v3, Groth16 in v4)
pub type JoinSplit = zcash_primitives::transaction::components::sprout::JsDescription;

pub use zcash_primitives::transaction::components::sprout::SproutProof;
