//! A tonic RPC server for Zebra's indexer API.

use std::sync::Arc;

use zebra_chain::{
    block,
    serialization::{BytesInDisplayOrder, ZcashDeserializeInto, ZcashSerialize},
};

#[cfg(test)]
mod tests;

pub mod methods;
pub mod server;

// The generated indexer proto
tonic::include_proto!("zebra.indexer.rpc");

pub(crate) const FILE_DESCRIPTOR_SET: &[u8] =
    tonic::include_file_descriptor_set!("indexer_descriptor");

impl BlockHashAndHeight {
    /// Create a new [`BlockHashAndHeight`] from a [`block::Hash`] and [`block::Height`].
    pub fn new(hash: block::Hash, block::Height(height): block::Height) -> Self {
        let hash = hash.bytes_in_display_order().to_vec();
        BlockHashAndHeight { hash, height }
    }

    /// Try to convert a [`BlockHashAndHeight`] into a tuple of a block hash and height.
    pub fn try_into_hash_and_height(self) -> Option<(block::Hash, block::Height)> {
        self.hash
            .try_into()
            .map(|bytes| block::Hash::from_bytes_in_display_order(&bytes))
            .map_err(|bytes: Vec<_>| {
                tracing::warn!(
                    "failed to convert BlockHash to Hash, unexpected len: {}",
                    bytes.len()
                )
            })
            .ok()
            .zip(self.height.try_into().ok())
    }
}

impl BlockAndHash {
    /// Creates a new [`BlockAndHash`] from a [`block::Hash`] and [`block::Height`].
    ///
    /// # Panics
    ///
    /// This function will panic if the block serialization fails (if the header version is invalid).
    pub fn new(hash: block::Hash, block: Arc<block::Block>) -> Self {
        BlockAndHash {
            hash: hash.bytes_in_display_order().to_vec(),
            data: block
                .zcash_serialize_to_vec()
                .expect("block serialization should not fail"),
        }
    }

    /// Creates a new [`BlockAndHash`] from a block's stored bytes, hashing its header.
    ///
    /// Only the header is parsed: the transactions are passed through as stored.
    ///
    /// # Errors
    ///
    /// Returns an error if the bytes do not start with a valid block header.
    pub fn from_serialized(
        block: block::SerializedBlock,
    ) -> Result<Self, zebra_chain::serialization::SerializationError> {
        let header: block::Header = block.as_ref().zcash_deserialize_into()?;
        Ok(BlockAndHash {
            hash: header.hash().bytes_in_display_order().to_vec(),
            data: block.into(),
        })
    }

    /// Try to convert a [`BlockAndHash`] into a tuple of a decoded block and hash.
    pub fn decode(self) -> Option<(block::Block, block::Hash)> {
        self.hash
            .try_into()
            .map(|bytes| block::Hash::from_bytes_in_display_order(&bytes))
            .map_err(|bytes: Vec<_>| {
                tracing::warn!(
                    "failed to convert BlockHash to Hash, unexpected len: {}",
                    bytes.len()
                )
            })
            .ok()
            .and_then(|hash| {
                self.data
                    .zcash_deserialize_into()
                    .map_err(|err| tracing::warn!(?err, "failed to deserialize block",))
                    .ok()
                    .zip(Some(hash))
            })
    }
}
