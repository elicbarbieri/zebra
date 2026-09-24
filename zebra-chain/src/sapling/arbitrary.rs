//! Randomised data generation for sapling types.

use group::{Group, GroupEncoding};
use jubjub::ExtendedPoint;
use rand::SeedableRng;
use rand_chacha::ChaChaRng;

use proptest::{array, collection::vec, prelude::*};
use sapling_crypto::bundle::{
    Authorized, OutputDescription, OutputDescriptionBytes, SpendDescription, SpendDescriptionBytes,
    OUTPUT_DESCRIPTION_V4_SIZE, SPEND_DESCRIPTION_V4_SIZE,
};
use zcash_protocol::value::ZatBalance;

use super::tree;
use crate::{amount::Amount, transaction::arbitrary::MAX_ARBITRARY_ITEMS};

/// Point-tier Sapling spend
pub type Spend = SpendDescription<Authorized>;

/// Point-tier Sapling bundle
pub type Bundle = sapling_crypto::Bundle<Authorized, ZatBalance>;

/// Bundle whose points all decompress
///
/// - `shared_anchor` = v5/v6 layout (one anchor for every spend)
pub fn bundle(shared_anchor: bool) -> BoxedStrategy<Bundle> {
    (
        any::<tree::Root>(),
        vec(
            (any::<tree::Root>(), spend_fields()),
            0..MAX_ARBITRARY_ITEMS,
        ),
        vec(output(), 0..MAX_ARBITRARY_ITEMS),
        any::<Amount>(),
        vec(any::<u8>(), 64),
    )
        .prop_filter_map(
            "a bundle needs a spend or an output",
            move |(shared, spends, outputs, value_balance, binding_sig)| {
                let spends = spends
                    .into_iter()
                    .map(|(own, fields)| {
                        spend_from(if shared_anchor { shared } else { own }, fields)
                    })
                    .collect();
                let binding_sig: [u8; 64] = binding_sig.try_into().expect("vec is 64 bytes");
                Bundle::from_parts(
                    spends,
                    outputs,
                    ZatBalance::from_i64(value_balance.into()).expect("Amount = ZatBalance range"),
                    Authorized {
                        binding_sig: redjubjub::Signature::from(binding_sig),
                    },
                )
            },
        )
        .boxed()
}

/// Spend with its own anchor, whose points all decompress
pub fn spend() -> impl Strategy<Value = Spend> {
    (any::<tree::Root>(), spend_fields()).prop_map(|(anchor, fields)| spend_from(anchor, fields))
}

/// `bundle` with its spends replaced and a zero value balance (`None` = no spends or outputs)
pub fn with_spends(bundle: &Bundle, spends: impl IntoIterator<Item = Spend>) -> Option<Bundle> {
    Bundle::from_parts(
        spends.into_iter().collect(),
        bundle.shielded_outputs().to_vec(),
        ZatBalance::zero(),
        *bundle.authorization(),
    )
}

/// `spend` revealing `nullifier` instead
pub fn with_nullifier(spend: &Spend, nullifier: sapling_crypto::Nullifier) -> Spend {
    Spend::from_parts(
        spend.cv().clone(),
        *spend.anchor(),
        nullifier,
        *spend.rk(),
        *spend.zkproof(),
        *spend.spend_auth_sig(),
    )
}

/// nullifier, valid `rk`, proof, spend auth sig
type SpendFields = ([u8; 32], [u8; 32], Vec<u8>, Vec<u8>);

fn spend_fields() -> impl Strategy<Value = SpendFields> {
    (
        array::uniform32(any::<u8>()),
        spend_auth_verification_key_bytes(),
        vec(any::<u8>(), 192),
        vec(any::<u8>(), 64),
    )
}

/// Generator `cv` (decompresses, not small order)
fn spend_from(anchor: tree::Root, (nullifier, rk, proof, sig): SpendFields) -> Spend {
    let mut encoding = [0u8; SPEND_DESCRIPTION_V4_SIZE];
    encoding[0..32].copy_from_slice(&generator_bytes());
    encoding[32..64].copy_from_slice(&<[u8; 32]>::from(anchor));
    encoding[64..96].copy_from_slice(&nullifier);
    encoding[96..128].copy_from_slice(&rk);
    encoding[128..320].copy_from_slice(&proof);
    encoding[320..].copy_from_slice(&sig);
    SpendDescriptionBytes::from_bytes(&encoding)
        .expect("tree::Root = canonical anchor")
        .decompress()
        .expect("generator cv and a derived rk decompress")
}

/// Generator `cv` and `epk`, zero `cmu` (canonical)
fn output() -> impl Strategy<Value = OutputDescription<[u8; 192]>> {
    (vec(any::<u8>(), 580 + 80), vec(any::<u8>(), 192)).prop_map(|(ciphertexts, proof)| {
        let mut encoding = [0u8; OUTPUT_DESCRIPTION_V4_SIZE];
        encoding[0..32].copy_from_slice(&generator_bytes());
        encoding[64..96].copy_from_slice(&generator_bytes());
        encoding[96..756].copy_from_slice(&ciphertexts);
        encoding[756..].copy_from_slice(&proof);
        OutputDescriptionBytes::from_bytes(&encoding)
            .expect("zero cmu is canonical")
            .decompress()
            .expect("generator cv and epk decompress")
    })
}

fn generator_bytes() -> [u8; 32] {
    ExtendedPoint::generator().to_bytes()
}

fn spend_auth_verification_key_bytes() -> impl Strategy<Value = [u8; 32]> {
    array::uniform32(any::<u8>()).prop_map(|seed| {
        let sk = redjubjub::SigningKey::<redjubjub::SpendAuth>::new(ChaChaRng::from_seed(seed));
        <[u8; 32]>::from(redjubjub::VerificationKey::<redjubjub::SpendAuth>::from(
            &sk,
        ))
    })
}

fn jubjub_base_strat() -> BoxedStrategy<jubjub::Base> {
    (vec(any::<u8>(), 64))
        .prop_map(|bytes| {
            let bytes = bytes.try_into().expect("vec is the correct length");
            jubjub::Base::from_bytes_wide(&bytes)
        })
        .boxed()
}

impl Arbitrary for tree::Root {
    type Parameters = ();

    fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
        jubjub_base_strat().prop_map(tree::Root).boxed()
    }

    type Strategy = BoxedStrategy<Self>;
}

impl Arbitrary for tree::legacy::Node {
    type Parameters = ();

    fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
        jubjub_base_strat()
            .prop_map(tree::legacy::Node::from)
            .boxed()
    }

    type Strategy = BoxedStrategy<Self>;
}

impl From<jubjub::Fq> for tree::legacy::Node {
    fn from(x: jubjub::Fq) -> Self {
        let node = sapling_crypto::Node::from_bytes(x.to_bytes());
        if node.is_some().into() {
            tree::legacy::Node(node.unwrap())
        } else {
            sapling_crypto::Node::from_bytes([0; 32]).unwrap().into()
        }
    }
}
