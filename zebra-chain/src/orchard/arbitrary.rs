//! Randomised data generation for Orchard types.

use group::{
    ff::{FromUniformBytes, PrimeField},
    CurveAffine, GroupEncoding,
};
use halo2::pasta::pallas;
use nonempty::NonEmpty;
use reddsa::{orchard::SpendAuth, SigningKey, VerificationKey, VerificationKeyBytes};

use ::orchard::{
    bundle::{Authorized, BundleVersion, Flags},
    primitives::redpallas,
    Proof, ACTION_DESCRIPTION_SIZE,
};
use proptest::{collection::vec, prelude::*};
use zcash_protocol::value::ZatBalance;

use super::{note, tree};
use crate::{
    amount::Amount, parameters::NetworkUpgrade, transaction::arbitrary::MAX_ARBITRARY_ITEMS,
};

/// Point-tier Orchard action
pub type Action = ::orchard::Action<redpallas::Signature<redpallas::SpendAuth>>;

/// Point-tier Orchard-protocol bundle
pub type Bundle = ::orchard::Bundle<Authorized, ZatBalance>;

/// Orchard-pool bundle at `network_upgrade` whose points all decompress (proof = noise of the
/// canonical length)
pub fn bundle(network_upgrade: NetworkUpgrade) -> BoxedStrategy<Bundle> {
    let bundle_version = network_upgrade
        .branch_id()
        .and_then(|branch_id| zcash_protocol::consensus::BranchId::try_from(branch_id).ok())
        .and_then(|branch_id| {
            zcash_primitives::transaction::components::orchard::bundle_version_for_branch(
                branch_id,
                ::orchard::ValuePool::Orchard,
            )
        })
        .expect("the Orchard pool is defined at network_upgrade");

    (
        vec(action(), 1..MAX_ARBITRARY_ITEMS),
        flags(bundle_version),
        any::<Amount>(),
        any::<tree::Root>(),
        binding_signature(),
    )
        .prop_flat_map(|(actions, flags, value_balance, anchor, binding_sig)| {
            let proof = vec(any::<u8>(), Proof::expected_proof_size(actions.len()));
            (
                Just((actions, flags, value_balance, anchor, binding_sig)),
                proof,
            )
        })
        .prop_map(
            move |((actions, flags, value_balance, anchor, binding_sig), proof)| {
                Bundle::try_from_parts(
                    NonEmpty::from_vec(actions).expect("strategy yields >= 1 action"),
                    flags,
                    ZatBalance::from_i64(value_balance.into()).expect("Amount = ZatBalance range"),
                    ::orchard::Anchor::from_bytes(anchor.into())
                        .expect("tree::Root = canonical pallas::Base"),
                    Authorized::from_parts(Proof::new(proof), binding_sig),
                    bundle_version,
                )
                .expect("canonical proof size")
            },
        )
        .boxed()
}

/// `bundle` with its actions replaced and a zero value balance (proof resized to stay canonical)
///
/// # Panics
///
/// If `actions` is empty.
pub fn with_actions(bundle: &Bundle, actions: impl IntoIterator<Item = Action>) -> Bundle {
    let actions = NonEmpty::from_vec(actions.into_iter().collect())
        .expect("an Orchard bundle needs an action");
    let proof = Proof::new(vec![0; Proof::expected_proof_size(actions.len())]);

    Bundle::try_from_parts(
        actions,
        *bundle.flags(),
        ZatBalance::zero(),
        *bundle.anchor(),
        Authorized::from_parts(proof, bundle.authorization().binding_signature().clone()),
        bundle.bundle_version(),
    )
    .expect("canonical proof size")
}

/// `action` revealing `nullifier` instead
pub fn with_nullifier(action: &Action, nullifier: ::orchard::note::Nullifier) -> Action {
    Action::from_parts(
        nullifier,
        action.rk().clone(),
        *action.cmx(),
        action.encrypted_note().clone(),
        action.cv_net().clone(),
        action.authorization().clone(),
    )
    .expect("rk unchanged")
}

/// Action with a valid `rk`, identity `cv_net` and generator `epk` (all decompress)
pub fn action() -> impl Strategy<Value = Action> {
    (
        any::<note::Nullifier>(),
        spend_auth_verification_key_bytes(),
        vec(any::<u8>(), 580 + 80),
        vec(any::<u8>(), 64),
    )
        .prop_map(|(nullifier, rk, ciphertexts, sig)| {
            let mut encoding = [0u8; ACTION_DESCRIPTION_SIZE];
            encoding[0..32].copy_from_slice(&pallas::Affine::identity().to_bytes());
            encoding[32..64].copy_from_slice(&<[u8; 32]>::from(nullifier));
            encoding[64..96].copy_from_slice(&<[u8; 32]>::from(rk));
            // cmx = 0 (canonical)
            encoding[128..160].copy_from_slice(&pallas::Affine::generator().to_bytes());
            encoding[160..].copy_from_slice(&ciphertexts);
            let sig: [u8; 64] = sig.try_into().expect("vec is the correct length");

            ::orchard::ActionBytes::from_bytes(&encoding)
                .expect("canonical nullifier and cmx")
                .with_authorization(redpallas::Signature::from(sig))
                .decompress()
                .expect("identity cv_net, derived rk and generator epk decompress")
        })
}

/// Flag bytes `bundle_version` can encode (bit 2 = Ironwood only)
fn flags(bundle_version: BundleVersion) -> impl Strategy<Value = Flags> {
    (0u8..8).prop_filter_map("flag byte not valid for this bundle version", move |byte| {
        Flags::from_byte(byte, bundle_version)
    })
}

fn binding_signature() -> impl Strategy<Value = redpallas::Signature<redpallas::Binding>> {
    vec(any::<u8>(), 64).prop_filter_map("zero binding signature", |bytes| {
        let sig: [u8; 64] = bytes.try_into().expect("vec is the correct length");
        (sig != [0u8; 64]).then(|| redpallas::Signature::from(sig))
    })
}

fn spend_auth_verification_key_bytes() -> impl Strategy<Value = VerificationKeyBytes<SpendAuth>> {
    vec(any::<u8>(), 64).prop_map(|bytes| {
        let bytes = bytes.try_into().expect("vec is the correct length");
        let sk_bytes = pallas::Scalar::from_uniform_bytes(&bytes).to_repr();
        let sk = SigningKey::from_bytes(&sk_bytes).expect("canonical scalar");
        VerificationKey::<SpendAuth>::from(&sk).into()
    })
}

impl Arbitrary for note::Nullifier {
    type Parameters = ();

    fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
        (vec(any::<u8>(), 64))
            .prop_map(|bytes| {
                let bytes = bytes.try_into().expect("vec is the correct length");
                Self::try_from(pallas::Scalar::from_uniform_bytes(&bytes).to_repr())
                    .expect("a valid generated nullifier")
            })
            .boxed()
    }

    type Strategy = BoxedStrategy<Self>;
}

fn pallas_base_strat() -> BoxedStrategy<pallas::Base> {
    (vec(any::<u8>(), 64))
        .prop_map(|bytes| {
            let bytes = bytes.try_into().expect("vec is the correct length");
            pallas::Base::from_uniform_bytes(&bytes)
        })
        .boxed()
}

impl Arbitrary for tree::Root {
    type Parameters = ();

    fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
        pallas_base_strat()
            .prop_map(|base| {
                Self::try_from(base.to_repr())
                    .expect("a valid generated Orchard note commitment tree root")
            })
            .boxed()
    }

    type Strategy = BoxedStrategy<Self>;
}

impl Arbitrary for tree::Node {
    type Parameters = ();

    fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
        pallas_base_strat()
            .prop_map(|base| {
                Self::try_from(base.to_repr())
                    .expect("a valid generated Orchard note commitment tree root")
            })
            .boxed()
    }

    type Strategy = BoxedStrategy<Self>;
}
