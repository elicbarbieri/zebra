//! Sapling-related functionality.
//!
//! These data structures enforce the *structural validity* of Sapling-related
//! consensus-critical objects.
//!
//! **Consensus rule**:
//!
//! These data structures ensure that [ZIP-216](https://zips.z.cash/zip-0216),
//! canonical Jubjub point encodings, are enforced everywhere where Jubjub
//! points occur, and non-canonical point encodings are rejected. This is
//! enforced by the jubjub crate, which is also used by the redjubjub crate.

mod note;

#[cfg(any(test, feature = "proptest-impl"))]
pub mod arbitrary;

pub mod tree;

pub use note::Nullifier;
