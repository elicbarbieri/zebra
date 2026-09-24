//! Contains impls of `ZcashSerialize`, `ZcashDeserialize` for all of the
//! transaction types, so that all of the serialization logic is in one place.

use std::{io, sync::Arc};

use hex::FromHex;

use crate::{
    block::MAX_BLOCK_BYTES,
    serialization::{
        ReadZcashExt, SerializationError, TrustedPreallocate, ZcashDeserialize, ZcashSerialize,
    },
};

use super::*;

impl<T: reddsa::SigType> ZcashSerialize for reddsa::Signature<T> {
    fn zcash_serialize<W: io::Write>(&self, mut writer: W) -> Result<(), io::Error> {
        writer.write_all(&<[u8; 64]>::from(*self)[..])?;
        Ok(())
    }
}

impl<T: reddsa::SigType> ZcashDeserialize for reddsa::Signature<T> {
    fn zcash_deserialize<R: io::Read>(mut reader: R) -> Result<Self, SerializationError> {
        Ok(reader.read_64_bytes()?.into())
    }
}

impl<T> ZcashDeserialize for Arc<T>
where
    T: ZcashDeserialize,
{
    fn zcash_deserialize<R: io::Read>(reader: R) -> Result<Self, SerializationError> {
        Ok(Arc::new(T::zcash_deserialize(reader)?))
    }
}

impl<T> ZcashSerialize for Arc<T>
where
    T: ZcashSerialize,
{
    fn zcash_serialize<W: io::Write>(&self, writer: W) -> Result<(), io::Error> {
        T::zcash_serialize(self, writer)
    }
}

/// A Tx Input must have an Outpoint (32 byte hash + 4 byte index), a 4 byte sequence number,
/// and a signature script, which always takes a min of 1 byte (for a length 0 script).
pub(crate) const MIN_TRANSPARENT_INPUT_SIZE: u64 = 32 + 4 + 4 + 1;

/// A Transparent output has an 8 byte value and script which takes a min of 1 byte.
pub(crate) const MIN_TRANSPARENT_OUTPUT_SIZE: u64 = 8 + 1;

/// All txs must have at least one input, a 4 byte locktime, and at least one output.
///
/// Shielded transfers are much larger than transparent transfers,
/// so this is the minimum transaction size.
pub const MIN_TRANSPARENT_TX_SIZE: u64 =
    MIN_TRANSPARENT_INPUT_SIZE + 4 + MIN_TRANSPARENT_OUTPUT_SIZE;

/// The minimum transaction size for v4 transactions.
///
/// v4 transactions also have an expiry height.
pub const MIN_TRANSPARENT_TX_V4_SIZE: u64 = MIN_TRANSPARENT_TX_SIZE + 4;

/// The minimum transaction size for v5 transactions.
///
/// v5 transactions also have an expiry height and a consensus branch ID.
pub const MIN_TRANSPARENT_TX_V5_SIZE: u64 = MIN_TRANSPARENT_TX_SIZE + 4 + 4;

/// No valid Zcash message contains more transactions than can fit in a single block
///
/// `tx` messages contain a single transaction, and `block` messages are limited to the maximum
/// block size.
impl TrustedPreallocate for CompressedTransaction {
    fn max_allocation() -> u64 {
        // A transparent transaction is the smallest transaction variant
        MAX_BLOCK_BYTES / MIN_TRANSPARENT_TX_SIZE
    }
}

/// The maximum number of inputs in a valid Zcash on-chain transaction.
///
/// If a transaction contains more inputs than can fit in maximally large block, it might be
/// valid on the network and in the mempool, but it can never be mined into a block. So
/// rejecting these large edge-case transactions can never break consensus.
impl TrustedPreallocate for transparent::Input {
    fn max_allocation() -> u64 {
        MAX_BLOCK_BYTES / MIN_TRANSPARENT_INPUT_SIZE
    }
}

/// The maximum number of outputs in a valid Zcash on-chain transaction.
///
/// If a transaction contains more outputs than can fit in maximally large block, it might be
/// valid on the network and in the mempool, but it can never be mined into a block. So
/// rejecting these large edge-case transactions can never break consensus.
impl TrustedPreallocate for transparent::Output {
    fn max_allocation() -> u64 {
        MAX_BLOCK_BYTES / MIN_TRANSPARENT_OUTPUT_SIZE
    }
}

/// A serialized transaction.
///
/// Stores bytes that are guaranteed to be deserializable into a [`CompressedTransaction`].
///
/// Sorts in lexicographic order of the transaction's serialized data.
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SerializedTransaction {
    bytes: Vec<u8>,
}

impl fmt::Display for SerializedTransaction {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&hex::encode(&self.bytes))
    }
}

impl fmt::Debug for SerializedTransaction {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // A transaction with a lot of transfers can be extremely long in logs.
        let mut data_truncated = hex::encode(&self.bytes);
        if data_truncated.len() > 1003 {
            let end = data_truncated.len() - 500;
            // Replace the middle bytes with "...", but leave 500 bytes on either side.
            // The data is hex, so this replacement won't panic.
            data_truncated.replace_range(500..=end, "...");
        }

        f.debug_tuple("SerializedTransaction")
            .field(&data_truncated)
            .finish()
    }
}

/// Build a [`SerializedTransaction`] by serializing a transaction.
///
/// - Concrete impls, not `Borrow` (a foreign type's blanket impl overlaps `From<Vec<u8>>`)
impl From<&CompressedTransaction> for SerializedTransaction {
    fn from(tx: &CompressedTransaction) -> Self {
        SerializedTransaction {
            bytes: tx
                .zcash_serialize_to_vec()
                .expect("Writing to a `Vec` should never fail"),
        }
    }
}

impl From<CompressedTransaction> for SerializedTransaction {
    fn from(tx: CompressedTransaction) -> Self {
        (&tx).into()
    }
}

impl From<Arc<CompressedTransaction>> for SerializedTransaction {
    fn from(tx: Arc<CompressedTransaction>) -> Self {
        tx.as_ref().into()
    }
}

impl From<&Arc<CompressedTransaction>> for SerializedTransaction {
    fn from(tx: &Arc<CompressedTransaction>) -> Self {
        tx.as_ref().into()
    }
}

/// Access the serialized bytes of a [`SerializedTransaction`].
impl AsRef<[u8]> for SerializedTransaction {
    fn as_ref(&self) -> &[u8] {
        self.bytes.as_ref()
    }
}

impl From<Vec<u8>> for SerializedTransaction {
    fn from(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }
}

impl FromHex for SerializedTransaction {
    type Error = <Vec<u8> as FromHex>::Error;

    fn from_hex<T: AsRef<[u8]>>(hex: T) -> Result<Self, Self::Error> {
        let bytes = <Vec<u8>>::from_hex(hex)?;

        Ok(bytes.into())
    }
}
