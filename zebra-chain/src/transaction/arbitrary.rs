//! Arbitrary data generation for transaction proptests

pub mod shielded;

pub use shielded::{
    fake_bundle_for_branch, fake_orchard_bundle, fake_orchard_bundle_duplicate_nullifiers,
    fake_orchard_bundle_with_note, fake_v6_transaction, insert_fake_orchard_shielded_data,
    outputs_enabled_flags, v6_ironwood_flags_offset, v6_orchard_flags_offset,
    with_garbage_orchard_authorization, with_orchard_flags, with_orchard_value_balance,
};

use std::{cmp::max, collections::HashMap, ops::Neg, sync::Arc};

use chrono::{TimeZone, Utc};
use proptest::{collection::vec, option, prelude::*};

use crate::{
    amount::{self, Amount, NegativeAllowed, NonNegative},
    block::{self, arbitrary::MAX_PARTIAL_CHAIN_BLOCKS},
    parameters::{Network, NetworkUpgrade},
    serialization::{self, ZcashDeserializeInto},
    transaction::{TransactionExt, TransactionTestExt},
    transparent,
    value_balance::{ValueBalance, ValueBalanceError},
    LedgerState,
};

use zcash_primitives::transaction::TxVersion;
use zcash_transparent;

use super::{CompressedTransaction, LockTime, UnminedTx, VerifiedUnminedTx};

/// Returns the librustzcash consensus branch ID for `network_upgrade`, falling back to
/// `fallback` when the upgrade has no branch ID that librustzcash recognises.
///
/// Shielded bundles must be built for the branch of the transaction that will carry them, since
/// the branch selects the bundle version and therefore which flags are representable. The
/// fallback must match the one the corresponding `Transaction::test_v*` constructor uses, or the
/// bundle would be built for a different branch than the transaction it ends up in. In
/// particular NU7's branch ID is behind an unstable feature, so a v6 transaction at NU7 falls
/// back to NU6.3 — the branch where v6 and the Ironwood pool were introduced.
fn branch_id_of(
    network_upgrade: NetworkUpgrade,
    fallback: zcash_protocol::consensus::BranchId,
) -> zcash_protocol::consensus::BranchId {
    network_upgrade
        .branch_id()
        .and_then(|cbid| zcash_protocol::consensus::BranchId::try_from(cbid).ok())
        .unwrap_or(fallback)
}

/// The maximum number of arbitrary transactions, inputs, or outputs.
///
/// This size is chosen to provide interesting behaviour, but not be too large
/// for debugging.
pub const MAX_ARBITRARY_ITEMS: usize = 4;

// TODO: if needed, fixup transaction outputs
//       (currently 0..=9 outputs, consensus rules require 1..)
/// Proptest strategies and value fixers for [`CompressedTransaction`]
pub trait TransactionArbitrary: Sized {
    /// Generate a proptest strategy for V1 Transactions
    fn v1_strategy(ledger_state: LedgerState) -> BoxedStrategy<Self>;

    /// Generate a proptest strategy for V2 Transactions
    ///
    /// Note: the new Transaction type doesn't support arbitrary Sprout JoinSplit data
    /// in proptest strategies, so this generates transparent-only V2 transactions.
    fn v2_strategy(ledger_state: LedgerState) -> BoxedStrategy<Self>;

    /// Generate a proptest strategy for V3 Transactions
    ///
    /// Note: the new Transaction type doesn't support arbitrary Sprout JoinSplit data
    /// in proptest strategies, so this generates transparent-only V3 transactions.
    fn v3_strategy(ledger_state: LedgerState) -> BoxedStrategy<Self>;

    /// Generate a proptest strategy for V4 Transactions
    ///
    /// Note: the new Transaction type doesn't support arbitrary Sapling/Sprout shielded
    /// data in proptest strategies, so this generates transparent-only V4 transactions.
    fn v4_strategy(ledger_state: LedgerState) -> BoxedStrategy<Self>;

    /// Generate a proptest strategy for V5 Transactions
    ///
    /// Note: the new Transaction type doesn't support arbitrary Sapling/Orchard shielded
    /// data in proptest strategies, so this generates transparent-only V5 transactions.
    fn v5_strategy(ledger_state: LedgerState) -> BoxedStrategy<Self>;

    /// Generate a proptest strategy for V6 Transactions
    ///
    /// Note: like [`Self::v5_strategy`], the new Transaction type doesn't support arbitrary
    /// shielded data in proptest strategies, so this generates transparent-only V6 transactions.
    fn v6_strategy(ledger_state: LedgerState) -> BoxedStrategy<Self>;

    /// Proptest Strategy for creating a Vector of transactions where the first
    /// transaction is always the only coinbase transaction
    fn vec_strategy(ledger_state: LedgerState, len: usize) -> BoxedStrategy<Vec<Arc<Self>>>;

    /// Apply `f` to the transparent output values in this transaction.
    ///
    /// Note: Sprout/sapling/orchard value mutations are not supported with
    /// the new Transaction type (proptest strategies generate transparent-only txs).
    fn for_each_value_mut<F>(&mut self, f: F)
    where
        F: FnMut(&mut Amount<NonNegative>);

    /// Apply `f` to the sapling value balance and orchard value balance.
    ///
    /// Note: Not implemented for the new Transaction type since proptest strategies
    /// generate transparent-only transactions. This is a no-op.
    fn for_each_value_balance_mut<F>(&mut self, f: F)
    where
        F: FnMut(&mut Amount<NegativeAllowed>);

    /// Fixup transparent values and shielded value balances,
    /// so that transaction and chain value pools won't overflow MAX_MONEY.
    ///
    /// These fixes are applied to coinbase and non-coinbase transactions.
    //
    // TODO: do we want to allow overflow, based on an arbitrary bool?
    fn fix_overflow(&mut self);

    /// Fixup transparent values and shielded value balances,
    /// so that this transaction passes the "non-negative chain value pool" checks.
    /// (These checks use the sum of unspent outputs for each transparent and shielded pool.)
    ///
    /// These fixes are applied to coinbase and non-coinbase transactions.
    ///
    /// `chain_value_pools` contains the chain value pool balances,
    /// as of the previous transaction in this block
    /// (or the last transaction in the previous block).
    ///
    /// `outputs` must contain all the [`transparent::Output`]s spent in this transaction.
    ///
    /// Currently, these fixes almost always leave some remaining value in each transparent
    /// and shielded chain value pool.
    ///
    /// Before fixing the chain value balances, this method calls `fix_overflow`
    /// to make sure that transaction and chain value pools don't overflow MAX_MONEY.
    ///
    /// After fixing the chain value balances, this method calls `fix_remaining_value`
    /// to fix the remaining value in the transaction value pool.
    ///
    /// Returns the remaining transaction value, and the updated chain value balances.
    ///
    /// # Panics
    ///
    /// If any spent [`transparent::Output`] is missing from
    /// [`transparent::OutPoint`]s.
    //
    // TODO: take some extra arbitrary flags, which select between zero and non-zero
    //       remaining value in each chain value pool
    fn fix_chain_value_pools(
        &mut self,
        chain_value_pools: ValueBalance<NonNegative>,
        outputs: &HashMap<transparent::OutPoint, transparent::Output>,
    ) -> Result<(Amount<NonNegative>, ValueBalance<NonNegative>), ValueBalanceError>;

    /// Fixup non-coinbase transparent values and shielded value balances,
    /// so that this transaction passes the "non-negative remaining transaction value"
    /// check. (This check uses the sum of inputs minus outputs.)
    ///
    /// Returns the remaining transaction value.
    ///
    /// `outputs` must contain all the [`transparent::Output`]s spent in this transaction.
    ///
    /// Currently, these fixes almost always leave some remaining value in the
    /// transaction value pool.
    ///
    /// # Panics
    ///
    /// If any spent [`transparent::Output`] is missing from
    /// [`transparent::OutPoint`]s.
    //
    // TODO: split this method up, after we've implemented chain value balance adjustments
    //
    // TODO: take an extra arbitrary bool, which selects between zero and non-zero
    //       remaining value in the transaction value pool
    fn fix_remaining_value(
        &mut self,
        outputs: &HashMap<transparent::OutPoint, transparent::Output>,
    ) -> Result<Amount<NonNegative>, ValueBalanceError>;

    /// Strategy for any transaction valid under `ledger_state`
    fn strategy(ledger_state: LedgerState) -> BoxedStrategy<Self>;
}

impl TransactionArbitrary for CompressedTransaction {
    fn v1_strategy(ledger_state: LedgerState) -> BoxedStrategy<Self> {
        (
            transparent::Input::vec_strategy(&ledger_state, MAX_ARBITRARY_ITEMS),
            vec(any::<transparent::Output>(), 0..MAX_ARBITRARY_ITEMS),
            any::<LockTime>(),
        )
            .prop_map(|(inputs, outputs, lock_time)| {
                CompressedTransaction::test_v1(inputs, outputs, lock_time)
            })
            .boxed()
    }

    fn v2_strategy(ledger_state: LedgerState) -> BoxedStrategy<Self> {
        (
            transparent::Input::vec_strategy(&ledger_state, MAX_ARBITRARY_ITEMS),
            vec(any::<transparent::Output>(), 0..MAX_ARBITRARY_ITEMS),
            any::<LockTime>(),
        )
            .prop_map(|(inputs, outputs, lock_time)| {
                CompressedTransaction::test_v2(inputs, outputs, lock_time)
            })
            .boxed()
    }

    fn v3_strategy(ledger_state: LedgerState) -> BoxedStrategy<Self> {
        (
            transparent::Input::vec_strategy(&ledger_state, MAX_ARBITRARY_ITEMS),
            vec(any::<transparent::Output>(), 0..MAX_ARBITRARY_ITEMS),
            any::<LockTime>(),
            any::<block::Height>(),
        )
            .prop_map(|(inputs, outputs, lock_time, expiry_height)| {
                CompressedTransaction::test_v3(inputs, outputs, lock_time, expiry_height)
            })
            .boxed()
    }

    fn v4_strategy(ledger_state: LedgerState) -> BoxedStrategy<Self> {
        (
            transparent::Input::vec_strategy(&ledger_state, MAX_ARBITRARY_ITEMS),
            vec(any::<transparent::Output>(), 0..MAX_ARBITRARY_ITEMS),
            any::<LockTime>(),
            any::<block::Height>(),
        )
            .prop_map(|(inputs, outputs, lock_time, expiry_height)| {
                CompressedTransaction::test_v4(inputs, outputs, lock_time, expiry_height)
            })
            .boxed()
    }

    fn v5_strategy(ledger_state: LedgerState) -> BoxedStrategy<Self> {
        (
            NetworkUpgrade::nu5_branch_id_strategy(),
            any::<LockTime>(),
            any::<block::Height>(),
            transparent::Input::vec_strategy(&ledger_state, MAX_ARBITRARY_ITEMS),
            vec(any::<transparent::Output>(), 0..MAX_ARBITRARY_ITEMS),
            option::of((1usize..=2usize, any::<u64>())),
        )
            .prop_map(
                move |(network_upgrade, lock_time, expiry_height, inputs, outputs, orchard)| {
                    let nu = if ledger_state.transaction_has_valid_network_upgrade() {
                        // Use the ledger's network upgrade if it has a consensus branch ID
                        // (V5 transactions can only embed known branch IDs).
                        let ledger_nu = ledger_state.network_upgrade();
                        if ledger_nu.branch_id().is_some() {
                            ledger_nu
                        } else {
                            network_upgrade
                        }
                    } else {
                        network_upgrade
                    };

                    // The genesis block must not contain any shielded data.
                    let orchard_bundle = (!ledger_state.height.is_min())
                        .then_some(orchard)
                        .flatten()
                        .and_then(|(n_actions, seed)| {
                            shielded::fake_bundle_for_branch(
                                branch_id_of(nu, zcash_protocol::consensus::BranchId::Nu5),
                                ::orchard::ValuePool::Orchard,
                                n_actions,
                                seed,
                            )
                        });

                    CompressedTransaction::test_v5_with_orchard(
                        nu,
                        inputs,
                        outputs,
                        lock_time,
                        expiry_height,
                        orchard_bundle,
                    )
                },
            )
            .boxed()
    }

    fn v6_strategy(ledger_state: LedgerState) -> BoxedStrategy<Self> {
        (
            NetworkUpgrade::nu6_3_branch_id_strategy(),
            any::<LockTime>(),
            any::<block::Height>(),
            transparent::Input::vec_strategy(&ledger_state, MAX_ARBITRARY_ITEMS),
            vec(any::<transparent::Output>(), 0..MAX_ARBITRARY_ITEMS),
            option::of((1usize..=2usize, any::<u64>())),
            option::of((1usize..=2usize, any::<u64>())),
        )
            .prop_map(
                move |(
                    network_upgrade,
                    lock_time,
                    expiry_height,
                    inputs,
                    outputs,
                    orchard,
                    ironwood,
                )| {
                    let nu = if ledger_state.transaction_has_valid_network_upgrade() {
                        let ledger_nu = ledger_state.network_upgrade();
                        if ledger_nu.branch_id().is_some() {
                            ledger_nu
                        } else {
                            network_upgrade
                        }
                    } else {
                        network_upgrade
                    };

                    // The genesis block must not contain any shielded data.
                    let is_genesis = ledger_state.height.is_min();

                    let orchard_bundle =
                        (!is_genesis)
                            .then_some(orchard)
                            .flatten()
                            .and_then(|(n_actions, seed)| {
                                shielded::fake_bundle_for_branch(
                                    branch_id_of(nu, zcash_protocol::consensus::BranchId::Nu6_3),
                                    ::orchard::ValuePool::Orchard,
                                    n_actions,
                                    seed,
                                )
                            });

                    // Offset the Ironwood seeds so the two pools in one transaction never share
                    // nullifiers (they are disjoint sets, but the tests read more clearly when
                    // the generated values differ too).
                    let ironwood_bundle = (!is_genesis).then_some(ironwood).flatten().and_then(
                        |(n_actions, seed)| {
                            shielded::fake_bundle_for_branch(
                                branch_id_of(nu, zcash_protocol::consensus::BranchId::Nu6_3),
                                ::orchard::ValuePool::Ironwood,
                                n_actions,
                                seed ^ 0xEEEE_0000_u64,
                            )
                        },
                    );

                    CompressedTransaction::test_v6_with_bundles(
                        nu,
                        inputs,
                        outputs,
                        lock_time,
                        expiry_height,
                        orchard_bundle,
                        ironwood_bundle,
                    )
                },
            )
            .boxed()
    }

    fn vec_strategy(mut ledger_state: LedgerState, len: usize) -> BoxedStrategy<Vec<Arc<Self>>> {
        // TODO: fixup coinbase miner subsidy
        let coinbase = CompressedTransaction::strategy(ledger_state.clone()).prop_map(Arc::new);
        ledger_state.has_coinbase = false;
        let remainder = vec(
            CompressedTransaction::strategy(ledger_state).prop_map(Arc::new),
            0..=len,
        );

        (coinbase, remainder)
            .prop_map(|(first, mut remainder)| {
                remainder.insert(0, first);
                remainder
            })
            .boxed()
    }

    fn for_each_value_mut<F>(&mut self, mut f: F)
    where
        F: FnMut(&mut Amount<NonNegative>),
    {
        let mut outputs = self.outputs();
        let mut changed = false;
        for output in &mut outputs {
            let old = output.value;
            f(&mut output.value);
            if output.value != old {
                changed = true;
            }
        }
        if changed {
            *self = self.clone().with_transparent_outputs(outputs);
        }
    }

    fn for_each_value_balance_mut<F>(&mut self, _f: F)
    where
        F: FnMut(&mut Amount<NegativeAllowed>),
    {
        // The shielded bundles our strategies build always have a zero value balance
        // (see `shielded::fake_bundle_for_branch`), so there is nothing to scale.
        // If a strategy ever generates a non-zero one, this must rebuild the bundle:
        // `zcash_primitives` bundles are immutable, so there is no value balance to take
        // a `&mut` to.
    }

    fn fix_overflow(&mut self) {
        fn scale_to_avoid_overflow<C: amount::Constraint>(amount: &mut Amount<C>)
        where
            Amount<C>: Copy,
        {
            // transparent, sprout, sapling, orchard, and ironwood
            const POOL_COUNT: u64 = 5;

            let max_arbitrary_items: u64 = MAX_ARBITRARY_ITEMS.try_into().unwrap();
            let max_partial_chain_blocks: u64 = MAX_PARTIAL_CHAIN_BLOCKS.try_into().unwrap();

            // inputs/joinsplits/spends|outputs/actions * pools * transactions
            let transaction_pool_scaling_divisor =
                max_arbitrary_items * POOL_COUNT * max_arbitrary_items;
            // inputs/joinsplits/spends|outputs/actions * transactions * blocks
            let chain_pool_scaling_divisor =
                max_arbitrary_items * max_arbitrary_items * max_partial_chain_blocks;
            let scaling_divisor = max(transaction_pool_scaling_divisor, chain_pool_scaling_divisor);

            *amount = (*amount / scaling_divisor).expect("divisor is not zero");
        }

        self.for_each_value_mut(scale_to_avoid_overflow);
        // Shielded value balances are zero in proptest transactions, no fixup needed.
    }

    fn fix_chain_value_pools(
        &mut self,
        chain_value_pools: ValueBalance<NonNegative>,
        outputs: &HashMap<transparent::OutPoint, transparent::Output>,
    ) -> Result<(Amount<NonNegative>, ValueBalance<NonNegative>), ValueBalanceError> {
        self.fix_overflow();

        // a temporary value used to check that inputs don't break the chain value balance
        // consensus rules
        let mut input_chain_value_pools = chain_value_pools;

        for input in self.inputs() {
            input_chain_value_pools = input_chain_value_pools
                .add_transparent_input(input, outputs)
                .expect("find_valid_utxo_for_spend only spends unspent transparent outputs");
        }

        // update the input chain value pools,
        // zeroing any inputs that would exceed the input value
        //
        // Note: Sprout, Sapling, Orchard, and Ironwood pool mutations are skipped here
        // because proptest strategies generate transparent-only transactions.
        // Those value balances are always zero and never exceed the chain pool.

        let remaining_transaction_value = self.fix_remaining_value(outputs)?;

        // check our calculations are correct
        let transaction_chain_value_pool_change =
            self
            .transparent_value_balance_from_outputs(outputs)
            .expect("chain value pool and remaining transaction value fixes produce valid transaction value balances")
            .neg();

        let chain_value_pools = chain_value_pools
            .add_transaction(self, outputs)
            .unwrap_or_else(|err| {
                panic!(
                    "unexpected chain value pool error: {err:?}, \n\
                     original chain value pools: {chain_value_pools:?}, \n\
                     transaction chain value change: {transaction_chain_value_pool_change:?}, \n\
                     input-only transaction chain value pools: {input_chain_value_pools:?}, \n\
                     calculated remaining transaction value: {remaining_transaction_value:?}",
                )
            });

        Ok((remaining_transaction_value, chain_value_pools))
    }

    fn fix_remaining_value(
        &mut self,
        outputs: &HashMap<transparent::OutPoint, transparent::Output>,
    ) -> Result<Amount<NonNegative>, ValueBalanceError> {
        if self.is_coinbase() {
            // TODO: if needed, fixup coinbase:
            // - miner subsidy
            // - founders reward or funding streams (hopefully not?)
            // - remaining transaction value

            // Act as if the generated test case spends all the miner subsidy, miner fees, and
            // founders reward / funding stream correctly.
            return Ok(Amount::zero());
        }

        let mut remaining_input_value = input_value_pool(self, outputs)?;

        // assign remaining input value to outputs,
        // zeroing any outputs that would exceed the input value
        let mut tx_outputs = self.outputs();
        let mut outputs_changed = false;
        for output in &mut tx_outputs {
            if remaining_input_value >= output.value {
                remaining_input_value = (remaining_input_value - output.value)
                    .expect("input >= output so result is always non-negative");
            } else {
                output.value = Amount::zero();
                outputs_changed = true;
            }
        }
        if outputs_changed {
            *self = self.clone().with_transparent_outputs(tx_outputs);
        }

        // Sprout, Sapling, Orchard, and Ironwood output values are zero in proptest
        // transactions, so there is nothing to fix up for the shielded pools.

        // check our calculations are correct
        let remaining_transaction_value = self
            .transparent_value_balance_from_outputs(outputs)
            .expect("chain is limited to MAX_MONEY")
            .remaining_transaction_value()
            .unwrap_or_else(|err| {
                panic!(
                    "unexpected remaining transaction value: {err:?}, \
                     calculated remaining input value: {remaining_input_value:?}"
                )
            });
        assert_eq!(
            remaining_input_value,
            remaining_transaction_value,
            "fix_remaining_value and remaining_transaction_value calculated different remaining values"
        );

        Ok(remaining_transaction_value)
    }

    fn strategy(ledger_state: LedgerState) -> BoxedStrategy<Self> {
        match ledger_state.transaction_version_override() {
            Some(1) => return Self::v1_strategy(ledger_state),
            Some(2) => return Self::v2_strategy(ledger_state),
            Some(3) => return Self::v3_strategy(ledger_state),
            Some(4) => return Self::v4_strategy(ledger_state),
            Some(5) => return Self::v5_strategy(ledger_state),
            Some(6) => return Self::v6_strategy(ledger_state),
            Some(_) => unreachable!("invalid transaction version in override"),
            None => {}
        }

        match ledger_state.network_upgrade() {
            NetworkUpgrade::Genesis | NetworkUpgrade::BeforeOverwinter => {
                Self::v1_strategy(ledger_state)
            }
            NetworkUpgrade::Overwinter => Self::v2_strategy(ledger_state),
            NetworkUpgrade::Sapling => Self::v3_strategy(ledger_state),
            NetworkUpgrade::Blossom | NetworkUpgrade::Heartwood | NetworkUpgrade::Canopy => {
                Self::v4_strategy(ledger_state)
            }
            NetworkUpgrade::Nu5
            | NetworkUpgrade::Nu6
            | NetworkUpgrade::Nu6_1
            | NetworkUpgrade::Nu6_2 => prop_oneof![
                Self::v4_strategy(ledger_state.clone()),
                Self::v5_strategy(ledger_state)
            ]
            .boxed(),

            // V6 transactions are only valid from NU6.3; v4 and v5 remain valid alongside them.
            NetworkUpgrade::Nu6_3 | NetworkUpgrade::Nu7 => prop_oneof![
                Self::v4_strategy(ledger_state.clone()),
                Self::v5_strategy(ledger_state.clone()),
                Self::v6_strategy(ledger_state)
            ]
            .boxed(),

            #[cfg(zcash_unstable = "zfuture")]
            NetworkUpgrade::ZFuture => prop_oneof![
                Self::v4_strategy(ledger_state.clone()),
                Self::v5_strategy(ledger_state)
            ]
            .boxed(),
        }
    }
}

/// Returns the total input value of this transaction's value pool.
///
/// This is the sum of transparent inputs, sprout input values,
/// and if positive, the sapling, orchard, and ironwood value balances.
///
/// `outputs` must contain all the [`transparent::Output`]s spent in this transaction.
fn input_value_pool(
    transaction: &CompressedTransaction,
    outputs: &HashMap<transparent::OutPoint, transparent::Output>,
) -> Result<Amount<NonNegative>, ValueBalanceError> {
    let transparent_inputs = transaction
        .inputs()
        .iter()
        .map(|input| input.value_from_outputs(outputs))
        .sum::<Result<Amount<NonNegative>, amount::Error>>()
        .map_err(ValueBalanceError::Transparent)?;
    // TODO: fix callers which cause overflows, check for:
    //       cached `outputs` that don't go through `fix_overflow`, and
    //       values much larger than MAX_MONEY
    //.expect("chain is limited to MAX_MONEY");

    // Proptest transactions don't have Sprout joinsplits, so sprout_inputs is always zero.
    let sprout_inputs = Amount::<NonNegative>::zero();

    // positive value balances add to the transaction value pool
    let sapling_input = transaction
        .sapling_value_balance()
        .sapling_amount()
        .constrain::<NonNegative>()
        .unwrap_or_else(|_| Amount::zero());

    let orchard_input = transaction
        .orchard_value_balance()
        .orchard_amount()
        .constrain::<NonNegative>()
        .unwrap_or_else(|_| Amount::zero());

    let ironwood_input = transaction
        .ironwood_value_balance()
        .ironwood_amount()
        .constrain::<NonNegative>()
        .unwrap_or_else(|_| Amount::zero());

    let transaction_input_value_pool =
        (transparent_inputs + sprout_inputs + sapling_input + orchard_input + ironwood_input)
            .expect("chain is limited to MAX_MONEY");

    Ok(transaction_input_value_pool)
}

impl Arbitrary for LockTime {
    type Parameters = ();

    fn arbitrary_with(_args: ()) -> Self::Strategy {
        prop_oneof![
            (block::Height::MIN.0..=LockTime::MAX_HEIGHT.0)
                .prop_map(|n| LockTime::Height(block::Height(n))),
            (LockTime::MIN_TIMESTAMP..=LockTime::MAX_TIMESTAMP).prop_map(|n| {
                LockTime::Time(
                    Utc.timestamp_opt(n, 0)
                        .single()
                        .expect("in-range number of seconds and valid nanosecond"),
                )
            })
        ]
        .boxed()
    }

    type Strategy = BoxedStrategy<Self>;
}

impl Arbitrary for UnminedTx {
    type Parameters = ();

    fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
        CompressedTransaction::strategy(LedgerState::default())
            .prop_map(|tx| UnminedTx::from(Arc::new(tx)))
            .boxed()
    }

    type Strategy = BoxedStrategy<Self>;
}

impl Arbitrary for VerifiedUnminedTx {
    type Parameters = ();

    fn arbitrary_with(_args: Self::Parameters) -> Self::Strategy {
        (
            any::<UnminedTx>(),
            any::<Amount<NonNegative>>(),
            any::<u32>(),
            any::<u32>(),
            any::<(u16, u16)>().prop_map(|(unpaid_actions, conventional_actions)| {
                (
                    unpaid_actions % conventional_actions.saturating_add(1),
                    conventional_actions,
                )
            }),
            any::<f32>(),
            serialization::arbitrary::datetime_u32(),
            any::<block::Height>(),
        )
            .prop_map(
                |(
                    transaction,
                    miner_fee,
                    sigops,
                    p2sh_sigops,
                    (conventional_actions, mut unpaid_actions),
                    fee_weight_ratio,
                    time,
                    height,
                )| {
                    if unpaid_actions > conventional_actions {
                        unpaid_actions = conventional_actions;
                    }

                    let conventional_actions = conventional_actions as u32;
                    let unpaid_actions = unpaid_actions as u32;

                    Self {
                        transaction,
                        miner_fee,
                        legacy_sigop_count: sigops,
                        p2sh_sigop_count: p2sh_sigops,
                        conventional_actions,
                        unpaid_actions,
                        fee_weight_ratio,
                        time: Some(time),
                        height: Some(height),
                        spent_outputs: std::sync::Arc::new(vec![]),
                    }
                },
            )
            .boxed()
    }
    type Strategy = BoxedStrategy<Self>;
}

// Utility functions

/// Convert `trans` into a fake v5 transaction.
///
/// Takes the transparent inputs/outputs from `trans` and builds a new V5
/// transaction at the given height. Used to test V5 sighash/serialization
/// with real transparent data from the test vector blocks.
pub fn transaction_to_fake_v5(
    trans: &CompressedTransaction,
    network: &Network,
    height: block::Height,
) -> CompressedTransaction {
    let block_nu = NetworkUpgrade::current(network, height);

    match trans.tx_version() {
        // V5+ already in the right format; just clone
        TxVersion::V5 | TxVersion::V6 => trans.clone(),
        // For V1-V4: build a V5 with the same transparent data
        _ => {
            use crate::transaction::compat;
            use zcash_primitives::transaction::{self as zp_tx, TxVersion};
            use zcash_protocol::consensus::BranchId;

            let branch_id = block_nu
                .branch_id()
                .and_then(|cbid| BranchId::try_from(cbid).ok())
                .unwrap_or(BranchId::Nu5);

            let inputs = trans.inputs();
            let outputs = trans.outputs();
            let vin = inputs.iter().map(compat::input_to_txin).collect();
            let vout = outputs.iter().map(compat::output_to_txout).collect();

            let lock_time_u32 =
                compat::lock_time_to_u32(&trans.lock_time().unwrap_or(LockTime::unlocked()));

            let transparent_bundle = Some(zcash_transparent::bundle::Bundle {
                vin,
                vout,
                authorization: zcash_transparent::bundle::Authorized,
            });

            // For V4, carry over the sapling bundle (already in zcash_primitives format)
            let sapling_bundle = trans.sapling_bundle().cloned();

            let tx_data = zp_tx::CompressedTransactionData::from_parts(
                TxVersion::V5,
                branch_id,
                lock_time_u32,
                zcash_protocol::consensus::BlockHeight::from_u32(height.0),
                transparent_bundle,
                None,
                sapling_bundle,
                None,
            );

            tx_data.freeze().expect("rebuilt from valid transaction")
        }
        // unreachable but suppress warning for non-nu7 builds
        #[allow(unreachable_patterns)]
        _ => trans.clone(),
    }
}

/// Iterate over transactions in the block test vectors for the specified `network`.
pub fn test_transactions(
    network: &Network,
) -> impl DoubleEndedIterator<Item = (block::Height, Arc<CompressedTransaction>)> {
    let blocks = network.block_iter();

    transactions_from_blocks(blocks)
}

/// Returns an iterator over V5 transactions extracted from the given blocks.
pub fn v5_transactions<'b>(
    blocks: impl DoubleEndedIterator<Item = (&'b u32, &'b &'static [u8])> + 'b,
) -> impl DoubleEndedIterator<Item = CompressedTransaction> + 'b {
    transactions_from_blocks(blocks).filter_map(|(_, tx)| match tx.tx_version() {
        TxVersion::V5 | TxVersion::V6 => Some((*tx).clone()),
        _ => None,
    })
}

/// Generate an iterator over ([`block::Height`], [`Arc<Transaction>`]).
pub fn transactions_from_blocks<'a>(
    blocks: impl DoubleEndedIterator<Item = (&'a u32, &'a &'static [u8])> + 'a,
) -> impl DoubleEndedIterator<Item = (block::Height, Arc<CompressedTransaction>)> + 'a {
    blocks.flat_map(|(&block_height, &block_bytes)| {
        let block = block_bytes
            .zcash_deserialize_into::<block::Block>()
            .expect("block is structurally valid");

        block
            .transactions
            .into_iter()
            .map(move |transaction| (block::Height(block_height), transaction))
    })
}
