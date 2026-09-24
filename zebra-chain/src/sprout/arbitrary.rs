//! Randomised data generation for sprout types.

use proptest::{array, collection::vec, prelude::*};
use zcash_primitives::transaction::components::{
    sprout::{NOTE_CIPHERTEXT_SIZE, PHGR_PROOF_SIZE},
    GROTH_PROOF_SIZE,
};
use zcash_protocol::value::Zatoshis;

use super::{JoinSplit, JoinSplitData, SproutProof};
use crate::{
    amount::{Amount, NonNegative},
    transaction::arbitrary::MAX_ARBITRARY_ITEMS,
};

/// JoinSplit section of 1.. JoinSplits (`use_groth` = v4 proofs)
pub fn joinsplit_data(use_groth: bool) -> impl Strategy<Value = JoinSplitData> {
    (
        vec(joinsplit(use_groth), 1..MAX_ARBITRARY_ITEMS),
        array::uniform32(any::<u8>()),
        vec(any::<u8>(), 64),
    )
        .prop_map(
            |(joinsplits, joinsplit_pubkey, joinsplit_sig)| JoinSplitData {
                joinsplits,
                joinsplit_pubkey,
                joinsplit_sig: joinsplit_sig.try_into().expect("vec is 64 bytes"),
            },
        )
}

/// JoinSplit with random fields (`use_groth` = v4 proof)
pub fn joinsplit(use_groth: bool) -> impl Strategy<Value = JoinSplit> {
    let proof_size = if use_groth {
        GROTH_PROOF_SIZE
    } else {
        PHGR_PROOF_SIZE
    };
    (
        any::<Amount<NonNegative>>(),
        any::<Amount<NonNegative>>(),
        array::uniform32(any::<u8>()),
        array::uniform2(array::uniform32(any::<u8>())),
        array::uniform2(array::uniform32(any::<u8>())),
        array::uniform32(any::<u8>()),
        array::uniform32(any::<u8>()),
        array::uniform2(array::uniform32(any::<u8>())),
        vec(any::<u8>(), proof_size),
        vec(any::<u8>(), 2 * NOTE_CIPHERTEXT_SIZE),
    )
        .prop_map(
            move |(
                vpub_old,
                vpub_new,
                anchor,
                nullifiers,
                commitments,
                ephemeral_key,
                random_seed,
                macs,
                proof,
                ciphertexts,
            )| {
                let proof = if use_groth {
                    SproutProof::Groth(proof.try_into().expect("GROTH_PROOF_SIZE bytes"))
                } else {
                    SproutProof::PHGR(proof.try_into().expect("PHGR_PROOF_SIZE bytes"))
                };
                let (first, second) = ciphertexts.split_at(NOTE_CIPHERTEXT_SIZE);
                JoinSplit::from_parts(
                    zatoshis(vpub_old),
                    zatoshis(vpub_new),
                    anchor,
                    nullifiers,
                    commitments,
                    ephemeral_key,
                    random_seed,
                    macs,
                    proof,
                    [
                        first.try_into().expect("NOTE_CIPHERTEXT_SIZE bytes"),
                        second.try_into().expect("NOTE_CIPHERTEXT_SIZE bytes"),
                    ],
                )
            },
        )
}

/// `joinsplit` with its public values replaced
pub fn with_values(
    joinsplit: &JoinSplit,
    vpub_old: Amount<NonNegative>,
    vpub_new: Amount<NonNegative>,
) -> JoinSplit {
    rebuild(joinsplit, |parts| {
        parts.vpub_old = zatoshis(vpub_old);
        parts.vpub_new = zatoshis(vpub_new);
    })
}

/// `joinsplit` revealing `nullifiers` instead
pub fn with_nullifiers(joinsplit: &JoinSplit, nullifiers: [[u8; 32]; 2]) -> JoinSplit {
    rebuild(joinsplit, |parts| parts.nullifiers = nullifiers)
}

/// `joinsplit` carrying `proof` instead
pub fn with_proof(joinsplit: &JoinSplit, proof: SproutProof) -> JoinSplit {
    rebuild(joinsplit, |parts| parts.proof = proof)
}

/// JoinSplit fields tests replace
struct Parts {
    vpub_old: Zatoshis,
    vpub_new: Zatoshis,
    nullifiers: [[u8; 32]; 2],
    proof: SproutProof,
}

fn rebuild(joinsplit: &JoinSplit, edit: impl FnOnce(&mut Parts)) -> JoinSplit {
    let mut parts = Parts {
        vpub_old: Zatoshis::try_from(joinsplit.vpub_old()).expect("non-negative on the wire"),
        vpub_new: Zatoshis::try_from(joinsplit.vpub_new()).expect("non-negative on the wire"),
        nullifiers: *joinsplit.nullifiers(),
        proof: joinsplit.proof().clone(),
    };
    edit(&mut parts);

    JoinSplit::from_parts(
        parts.vpub_old,
        parts.vpub_new,
        *joinsplit.anchor(),
        parts.nullifiers,
        *joinsplit.commitments(),
        *joinsplit.ephemeral_key(),
        *joinsplit.random_seed(),
        *joinsplit.macs(),
        parts.proof,
        *joinsplit.ciphertexts(),
    )
}

fn zatoshis(amount: Amount<NonNegative>) -> Zatoshis {
    amount
        .try_into()
        .expect("Amount<NonNegative> and Zatoshis share the MAX_MONEY range")
}
