//! Errors that can occur inside any `zebra-chain` submodule.

use std::{io, sync::Arc};
use thiserror::Error;
use zcash_protocol::value::BalanceError;

/// `zebra-chain`'s errors
#[derive(Clone, Error, Debug)]
pub enum Error {
    /// Invalid consensus branch ID.
    #[error("invalid consensus branch id")]
    InvalidConsensusBranchId,

    /// The error type for I/O operations of the `Read`, `Write`, `Seek`, and associated traits.
    #[error(transparent)]
    Io(#[from] Arc<io::Error>),

    /// The transaction is missing a network upgrade.
    #[error("the transaction is missing a network upgrade")]
    MissingNetworkUpgrade,

    /// Invalid amount.
    #[error(transparent)]
    Amount(#[from] BalanceError),

    /// Zebra's type could not be converted to its librustzcash equivalent.
    #[error("Zebra's type could not be converted to its librustzcash equivalent: {0}")]
    Conversion(String),

    /// Sapling/Orchard-protocol point-encoding rule broken (checked on decompress, not parse)
    #[error("invalid point encoding: {0}")]
    InvalidPointEncoding(Arc<zcash_primitives::transaction::DecompressionError>),
}

impl From<zcash_primitives::transaction::DecompressionError> for Error {
    fn from(value: zcash_primitives::transaction::DecompressionError) -> Self {
        Error::InvalidPointEncoding(Arc::new(value))
    }
}

/// Allow converting `io::Error` to `Error`; we need this since we
/// use `Arc<io::Error>` in `Error::Conversion`.
impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Arc::new(value).into()
    }
}

// We need to implement this manually because io::Error does not implement
// PartialEq.
impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        match self {
            Error::InvalidConsensusBranchId => matches!(other, Error::InvalidConsensusBranchId),
            Error::Io(e) => {
                if let Error::Io(o) = other {
                    // Not perfect, but good enough for testing, which
                    // is the main purpose for our usage of PartialEq for errors
                    e.to_string() == o.to_string()
                } else {
                    false
                }
            }
            Error::MissingNetworkUpgrade => matches!(other, Error::MissingNetworkUpgrade),
            Error::Amount(e) => matches!(other, Error::Amount(o) if e == o),
            Error::Conversion(e) => matches!(other, Error::Conversion(o) if e == o),
            Error::InvalidPointEncoding(e) => {
                matches!(other, Error::InvalidPointEncoding(o) if e.to_string() == o.to_string())
            }
        }
    }
}

impl Eq for Error {}
