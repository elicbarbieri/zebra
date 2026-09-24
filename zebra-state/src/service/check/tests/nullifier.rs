//! Randomised property tests for nullifier contextual validation

use std::{env, sync::Arc};

use itertools::Itertools;
use proptest::prelude::*;

use zebra_chain::{
    amount::Amount,
    block::{Block, Height},
    orchard,
    parameters::NetworkUpgrade::Nu5,
    sapling,
    serialization::ZcashDeserializeInto,
    sprout,
    transaction::{CompressedTransaction, LockTime, TransactionTestExt},
};

use crate::{
    arbitrary::Prepare,
    service::{
        check::nullifier::tx_no_duplicates_in_chain, read, write::validate_and_commit_non_finalized,
    },
    tests::setup::{new_state_with_mainnet_genesis, transaction_v4_from_coinbase},
    CheckpointVerifiedBlock,
    ValidateContextError::{
        DuplicateOrchardNullifier, DuplicateSaplingNullifier, DuplicateSproutNullifier,
    },
};

// These tests use the `Arbitrary` trait to easily generate complex types,
// then modify those types to cause an error (or to ensure success).
//
// We could use mainnet or testnet blocks in these tests,
// but the differences shouldn't matter,
// because we're only interested in spend validation,
// (and passing various other state checks).

const DEFAULT_NULLIFIER_PROPTEST_CASES: u32 = 2;

proptest! {
    #![proptest_config(
        proptest::test_runner::Config::with_cases(env::var("PROPTEST_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_NULLIFIER_PROPTEST_CASES))
    )]

    // sprout

    /// Make sure an arbitrary sprout nullifier is accepted by state contextual validation.
    ///
    /// This test makes sure there are no spurious rejections that might hide bugs in the other tests.
    /// (And that the test infrastructure generally works.)
    #[test]
    fn accept_distinct_arbitrary_sprout_nullifiers_in_one_block(
        joinsplit in sprout::arbitrary::joinsplit(true),
        joinsplit_data in sprout::arbitrary::joinsplit_data(true),
        use_finalized_state in any::<bool>(),
    ) {
        let _init_guard = zebra_test::init();

        let mut block1 = zebra_test::vectors::BLOCK_MAINNET_1_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");

        let mut nullifiers = *joinsplit.nullifiers();
        make_distinct_nullifiers(&mut nullifiers);
        let joinsplit = sprout::arbitrary::with_nullifiers(&joinsplit, nullifiers);
        let expected_nullifiers = nullifiers.map(sprout::Nullifier::from);

        let transaction = transaction_v4_with_joinsplit_data(joinsplit_data, [joinsplit]);

        // convert the coinbase transaction to a version that the non-finalized state will accept
        block1.transactions[0] = transaction_v4_from_coinbase(&block1.transactions[0]).into();

        block1.transactions.push(transaction.into());

        let (mut finalized_state, mut non_finalized_state, _genesis) = new_state_with_mainnet_genesis();

        // Allows anchor checks to pass
        finalized_state.populate_with_anchors(&block1);

        let previous_mem = non_finalized_state.clone();

        // randomly choose to commit the block to the finalized or non-finalized state
        if use_finalized_state {
            let block1 = CheckpointVerifiedBlock::from(Arc::new(block1));
            let commit_result = finalized_state.commit_finalized_direct(block1.clone().into(), None, "test");

            // the block was committed
            prop_assert_eq!(Some((Height(1), block1.hash)), read::best_tip(&non_finalized_state, &finalized_state.db));
            prop_assert!(commit_result.is_ok());

            // the non-finalized state didn't change
            prop_assert!(non_finalized_state.eq_internal_state(&previous_mem));

            // the finalized state has the nullifiers
            prop_assert!(finalized_state
                .contains_sprout_nullifier(&expected_nullifiers[0]));
            prop_assert!(finalized_state
                .contains_sprout_nullifier(&expected_nullifiers[1]));
        } else {
            let block1 = Arc::new(block1).prepare();
            let commit_result = validate_and_commit_non_finalized(
                &finalized_state.db,
                &mut non_finalized_state,
                block1.clone()
            );

            // the block was committed
            prop_assert_eq!(commit_result, Ok(()));
            prop_assert_eq!(Some((Height(1), block1.hash)), read::best_tip(&non_finalized_state, &finalized_state.db));

            // the block data is in the non-finalized state
            prop_assert!(!non_finalized_state.eq_internal_state(&previous_mem));

            // the non-finalized state has the nullifiers
            prop_assert_eq!(non_finalized_state.chain_count(), 1);
            prop_assert!(non_finalized_state
                .best_contains_sprout_nullifier(&expected_nullifiers[0]));
            prop_assert!(non_finalized_state
                .best_contains_sprout_nullifier(&expected_nullifiers[1]));
        }
    }

    /// Make sure duplicate sprout nullifiers are rejected by state contextual validation,
    /// if they come from the same JoinSplit.
    #[test]
    fn reject_duplicate_sprout_nullifiers_in_joinsplit(
        joinsplit in sprout::arbitrary::joinsplit(true),
        joinsplit_data in sprout::arbitrary::joinsplit_data(true),
    ) {
        let _init_guard = zebra_test::init();

        let mut block1 = zebra_test::vectors::BLOCK_MAINNET_1_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");

        // create a double-spend within the same joinsplit
        // this might not actually be valid under the nullifier generation consensus rules
        let mut nullifiers = *joinsplit.nullifiers();
        nullifiers[1] = nullifiers[0];
        let joinsplit = sprout::arbitrary::with_nullifiers(&joinsplit, nullifiers);
        let duplicate_nullifier = sprout::Nullifier::from(nullifiers[0]);

        let transaction = transaction_v4_with_joinsplit_data(joinsplit_data, [joinsplit]);

        block1.transactions[0] = transaction_v4_from_coinbase(&block1.transactions[0]).into();

        block1.transactions.push(transaction.into());

            let (finalized_state, mut non_finalized_state, genesis) = new_state_with_mainnet_genesis();

        // Allows anchor checks to pass
        finalized_state.populate_with_anchors(&block1);

        let previous_mem = non_finalized_state.clone();

        let block1 = Arc::new(block1).prepare();
        let commit_result = validate_and_commit_non_finalized(
            &finalized_state.db,
            &mut non_finalized_state,
            block1
        );

        // if the random proptest data produces other errors,
        // we might need to just check `is_err()` here
        prop_assert_eq!(
            commit_result,
            Err(DuplicateSproutNullifier {
                nullifier: duplicate_nullifier,
                in_finalized_state: false,
            })
        );
        // block was rejected
        prop_assert_eq!(Some((Height(0), genesis.hash)), read::best_tip(&non_finalized_state, &finalized_state.db));
        prop_assert!(non_finalized_state.eq_internal_state(&previous_mem));
    }

    /// Make sure duplicate sprout nullifiers are rejected by state contextual validation,
    /// if they come from different JoinSplits in the same JoinSplitData/Transaction.
    #[test]
    fn reject_duplicate_sprout_nullifiers_in_transaction(
        joinsplit1 in sprout::arbitrary::joinsplit(true),
        joinsplit2 in sprout::arbitrary::joinsplit(true),
        joinsplit_data in sprout::arbitrary::joinsplit_data(true),
    ) {
        let _init_guard = zebra_test::init();

        let mut block1 = zebra_test::vectors::BLOCK_MAINNET_1_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");

        let mut nullifiers1 = *joinsplit1.nullifiers();
        let mut nullifiers2 = *joinsplit2.nullifiers();
        make_distinct_nullifiers(nullifiers1.iter_mut().chain(nullifiers2.iter_mut()));

        // create a double-spend across two joinsplits
        nullifiers2[0] = nullifiers1[0];
        let joinsplit1 = sprout::arbitrary::with_nullifiers(&joinsplit1, nullifiers1);
        let joinsplit2 = sprout::arbitrary::with_nullifiers(&joinsplit2, nullifiers2);
        let duplicate_nullifier = sprout::Nullifier::from(nullifiers1[0]);

        let transaction =
            transaction_v4_with_joinsplit_data(joinsplit_data, [joinsplit1, joinsplit2]);

        block1.transactions[0] = transaction_v4_from_coinbase(&block1.transactions[0]).into();

        block1.transactions.push(transaction.into());

            let (finalized_state, mut non_finalized_state, genesis) = new_state_with_mainnet_genesis();

        // Allows anchor checks to pass
        finalized_state.populate_with_anchors(&block1);

        let previous_mem = non_finalized_state.clone();

        let block1 = Arc::new(block1).prepare();
        let commit_result = validate_and_commit_non_finalized(
            &finalized_state.db,
            &mut non_finalized_state,
            block1
        );

        prop_assert_eq!(
            commit_result,
            Err(DuplicateSproutNullifier {
                nullifier: duplicate_nullifier,
                in_finalized_state: false,
            })
        );
        prop_assert_eq!(Some((Height(0), genesis.hash)), read::best_tip(&non_finalized_state, &finalized_state.db));
        prop_assert!(non_finalized_state.eq_internal_state(&previous_mem));
    }

    /// Make sure duplicate sprout nullifiers are rejected by state contextual validation,
    /// if they come from different transactions in the same block.
    #[test]
    fn reject_duplicate_sprout_nullifiers_in_block(
        joinsplit1 in sprout::arbitrary::joinsplit(true),
        joinsplit2 in sprout::arbitrary::joinsplit(true),
        joinsplit_data1 in sprout::arbitrary::joinsplit_data(true),
        joinsplit_data2 in sprout::arbitrary::joinsplit_data(true),
    ) {
        let _init_guard = zebra_test::init();

        let mut block1 = zebra_test::vectors::BLOCK_MAINNET_1_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");

        let mut nullifiers1 = *joinsplit1.nullifiers();
        let mut nullifiers2 = *joinsplit2.nullifiers();
        make_distinct_nullifiers(nullifiers1.iter_mut().chain(nullifiers2.iter_mut()));

        // create a double-spend across two transactions
        nullifiers2[0] = nullifiers1[0];
        let joinsplit1 = sprout::arbitrary::with_nullifiers(&joinsplit1, nullifiers1);
        let joinsplit2 = sprout::arbitrary::with_nullifiers(&joinsplit2, nullifiers2);
        let duplicate_nullifier = sprout::Nullifier::from(nullifiers1[0]);

        let transaction1 = transaction_v4_with_joinsplit_data(joinsplit_data1, [joinsplit1]);
        let transaction2 = transaction_v4_with_joinsplit_data(joinsplit_data2, [joinsplit2]);

        block1.transactions[0] = transaction_v4_from_coinbase(&block1.transactions[0]).into();

        block1
            .transactions
            .extend([transaction1.into(), transaction2.into()]);

            let (finalized_state, mut non_finalized_state, genesis) = new_state_with_mainnet_genesis();

        // Allows anchor checks to pass
        finalized_state.populate_with_anchors(&block1);

        let previous_mem = non_finalized_state.clone();

        let block1 = Arc::new(block1).prepare();
        let commit_result = validate_and_commit_non_finalized(
            &finalized_state.db,
            &mut non_finalized_state,
            block1
        );

        prop_assert_eq!(
            commit_result,
            Err(DuplicateSproutNullifier {
                nullifier: duplicate_nullifier,
                in_finalized_state: false,
            })
        );
        prop_assert_eq!(Some((Height(0), genesis.hash)), read::best_tip(&non_finalized_state, &finalized_state.db));
        prop_assert!(non_finalized_state.eq_internal_state(&previous_mem));
    }

    /// Make sure duplicate sprout nullifiers are rejected by state contextual validation,
    /// if they come from different blocks in the same chain.
    #[test]
    fn reject_duplicate_sprout_nullifiers_in_chain(
        joinsplit1 in sprout::arbitrary::joinsplit(true),
        joinsplit2 in sprout::arbitrary::joinsplit(true),
        joinsplit_data1 in sprout::arbitrary::joinsplit_data(true),
        joinsplit_data2 in sprout::arbitrary::joinsplit_data(true),
        duplicate_in_finalized_state in any::<bool>(),
    ) {
        let _init_guard = zebra_test::init();

        let mut block1 = zebra_test::vectors::BLOCK_MAINNET_1_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");
        let mut block2 = zebra_test::vectors::BLOCK_MAINNET_2_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");

        let mut nullifiers1 = *joinsplit1.nullifiers();
        let mut nullifiers2 = *joinsplit2.nullifiers();
        make_distinct_nullifiers(nullifiers1.iter_mut().chain(nullifiers2.iter_mut()));
        let expected_nullifiers = nullifiers1.map(sprout::Nullifier::from);

        // create a double-spend across two blocks
        nullifiers2[0] = nullifiers1[0];
        let joinsplit1 = sprout::arbitrary::with_nullifiers(&joinsplit1, nullifiers1);
        let joinsplit2 = sprout::arbitrary::with_nullifiers(&joinsplit2, nullifiers2);
        let duplicate_nullifier = sprout::Nullifier::from(nullifiers1[0]);

        let transaction1 = Arc::new(transaction_v4_with_joinsplit_data(joinsplit_data1, [joinsplit1]));
        let transaction2 = transaction_v4_with_joinsplit_data(joinsplit_data2, [joinsplit2]);

        block1.transactions[0] = transaction_v4_from_coinbase(&block1.transactions[0]).into();
        block2.transactions[0] = transaction_v4_from_coinbase(&block2.transactions[0]).into();

        block1.transactions.push(transaction1.clone());
        block2.transactions.push(transaction2.into());

        let (mut finalized_state, mut non_finalized_state, _genesis) = new_state_with_mainnet_genesis();

        // Allows anchor checks to pass
        finalized_state.populate_with_anchors(&block1);
        finalized_state.populate_with_anchors(&block2);

        let mut previous_mem = non_finalized_state.clone();

        // makes sure there are no spurious rejections that might hide bugs in `tx_no_duplicates_in_chain`
        let check_tx_no_duplicates_in_chain =
            tx_no_duplicates_in_chain(&finalized_state.db, non_finalized_state.best_chain(), &transaction1);
        prop_assert!(check_tx_no_duplicates_in_chain.is_ok());

        let block1_hash;
        // randomly choose to commit the next block to the finalized or non-finalized state
        if duplicate_in_finalized_state {
            let block1 = CheckpointVerifiedBlock::from(Arc::new(block1));
            let commit_result = finalized_state.commit_finalized_direct(block1.clone().into(), None, "test");

            prop_assert_eq!(Some((Height(1), block1.hash)), read::best_tip(&non_finalized_state, &finalized_state.db));
            prop_assert!(commit_result.is_ok());
            prop_assert!(non_finalized_state.eq_internal_state(&previous_mem));
            prop_assert!(finalized_state
                .contains_sprout_nullifier(&expected_nullifiers[0]));
            prop_assert!(finalized_state
                .contains_sprout_nullifier(&expected_nullifiers[1]));

            block1_hash = block1.hash;
        } else {
            let block1 = Arc::new(block1).prepare();
            let commit_result = validate_and_commit_non_finalized(
                &finalized_state.db,
                &mut non_finalized_state,
                block1.clone()
            );

            prop_assert_eq!(commit_result, Ok(()));
            prop_assert_eq!(Some((Height(1), block1.hash)), read::best_tip(&non_finalized_state, &finalized_state.db));
            prop_assert!(!non_finalized_state.eq_internal_state(&previous_mem));
            prop_assert!(non_finalized_state
                .best_contains_sprout_nullifier(&expected_nullifiers[0]));
            prop_assert!(non_finalized_state
                .best_contains_sprout_nullifier(&expected_nullifiers[1]));

            block1_hash = block1.hash;
            previous_mem = non_finalized_state.clone();
        }

        let block2 = Arc::new(block2).prepare();
        let commit_result = validate_and_commit_non_finalized(
            &finalized_state.db,
            &mut non_finalized_state,
            block2
        );

        prop_assert_eq!(
            commit_result,
            Err(DuplicateSproutNullifier {
                nullifier: duplicate_nullifier,
                in_finalized_state: duplicate_in_finalized_state,
            })
        );

        let check_tx_no_duplicates_in_chain =
            tx_no_duplicates_in_chain(&finalized_state.db, non_finalized_state.best_chain(), &transaction1);

        prop_assert_eq!(
            check_tx_no_duplicates_in_chain,
            Err(DuplicateSproutNullifier {
                nullifier: duplicate_nullifier,
                in_finalized_state: duplicate_in_finalized_state,
            })
        );

        prop_assert_eq!(Some((Height(1), block1_hash)), read::best_tip(&non_finalized_state, &finalized_state.db));
        prop_assert!(non_finalized_state.eq_internal_state(&previous_mem));
    }

    // sapling

    /// Make sure an arbitrary sapling nullifier is accepted by state contextual validation.
    ///
    /// This test makes sure there are no spurious rejections that might hide bugs in the other tests.
    /// (And that the test infrastructure generally works.)
    #[test]
    fn accept_distinct_arbitrary_sapling_nullifiers_in_one_block(
        spend in sapling::arbitrary::spend(),
        sapling_shielded_data in sapling::arbitrary::bundle(false),
        use_finalized_state in any::<bool>(),
    ) {
        let _init_guard = zebra_test::init();

        let mut block1 = zebra_test::vectors::BLOCK_MAINNET_1_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");

        let expected_nullifier = sapling::Nullifier::from(spend.nullifier().0);

        let transaction =
            transaction_v4_with_sapling_shielded_data(sapling_shielded_data, [spend]);

        // convert the coinbase transaction to a version that the non-finalized state will accept
        block1.transactions[0] = transaction_v4_from_coinbase(&block1.transactions[0]).into();

        block1.transactions.push(transaction.into());

        let (mut finalized_state, mut non_finalized_state, _genesis) = new_state_with_mainnet_genesis();

        // Allows anchor checks to pass
        finalized_state.populate_with_anchors(&block1);

        let previous_mem = non_finalized_state.clone();

        // randomly choose to commit the block to the finalized or non-finalized state
        if use_finalized_state {
            let block1 = CheckpointVerifiedBlock::from(Arc::new(block1));
            let commit_result = finalized_state.commit_finalized_direct(block1.clone().into(),None,  "test");

            prop_assert_eq!(Some((Height(1), block1.hash)), read::best_tip(&non_finalized_state, &finalized_state.db));
            prop_assert!(commit_result.is_ok());
            prop_assert!(non_finalized_state.eq_internal_state(&previous_mem));
            prop_assert!(finalized_state.contains_sapling_nullifier(&expected_nullifier));
        } else {
            let block1 = Arc::new(block1).prepare();
            let commit_result = validate_and_commit_non_finalized(
                &finalized_state.db,
                &mut non_finalized_state,
                block1.clone()
            );

            prop_assert_eq!(commit_result, Ok(()));
            prop_assert_eq!(Some((Height(1), block1.hash)), read::best_tip(&non_finalized_state, &finalized_state.db));
            prop_assert!(!non_finalized_state.eq_internal_state(&previous_mem));
            prop_assert!(non_finalized_state
                .best_contains_sapling_nullifier(&expected_nullifier));
        }
    }

    /// Make sure duplicate sapling nullifiers are rejected by state contextual validation,
    /// if they come from different Spends in the same sapling::ShieldedData/Transaction.
    #[test]
    fn reject_duplicate_sapling_nullifiers_in_transaction(
        spend1 in sapling::arbitrary::spend(),
        mut spend2 in sapling::arbitrary::spend(),
        sapling_shielded_data in sapling::arbitrary::bundle(false),
    ) {
        let _init_guard = zebra_test::init();

        let mut block1 = zebra_test::vectors::BLOCK_MAINNET_1_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");

        // create a double-spend across two spends
        let duplicate_nullifier = sapling::Nullifier::from(spend1.nullifier().0);
        spend2 = sapling::arbitrary::with_nullifier(&spend2, *spend1.nullifier());

        let transaction = transaction_v4_with_sapling_shielded_data(
            sapling_shielded_data,
            [spend1, spend2],
        );

        block1.transactions[0] = transaction_v4_from_coinbase(&block1.transactions[0]).into();

        block1.transactions.push(transaction.into());

            let (finalized_state, mut non_finalized_state, genesis) = new_state_with_mainnet_genesis();

        // Allows anchor checks to pass
        finalized_state.populate_with_anchors(&block1);

        let previous_mem = non_finalized_state.clone();

        let block1 = Arc::new(block1).prepare();
        let commit_result = validate_and_commit_non_finalized(
            &finalized_state.db,
            &mut non_finalized_state,
            block1
        );

        prop_assert_eq!(
            commit_result,
            Err(DuplicateSaplingNullifier {
                nullifier: duplicate_nullifier,
                in_finalized_state: false,
            })
        );
        prop_assert_eq!(Some((Height(0), genesis.hash)), read::best_tip(&non_finalized_state, &finalized_state.db));
        prop_assert!(non_finalized_state.eq_internal_state(&previous_mem));
    }

    /// Make sure duplicate sapling nullifiers are rejected by state contextual validation,
    /// if they come from different transactions in the same block.
    #[test]
    fn reject_duplicate_sapling_nullifiers_in_block(
        spend1 in sapling::arbitrary::spend(),
        mut spend2 in sapling::arbitrary::spend(),
        sapling_shielded_data1 in sapling::arbitrary::bundle(false),
        sapling_shielded_data2 in sapling::arbitrary::bundle(false),
    ) {
        let _init_guard = zebra_test::init();

        let mut block1 = zebra_test::vectors::BLOCK_MAINNET_1_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");

        // create a double-spend across two transactions
        let duplicate_nullifier = sapling::Nullifier::from(spend1.nullifier().0);
        spend2 = sapling::arbitrary::with_nullifier(&spend2, *spend1.nullifier());

        let transaction1 =
            transaction_v4_with_sapling_shielded_data(sapling_shielded_data1, [spend1]);
        let transaction2 =
            transaction_v4_with_sapling_shielded_data(sapling_shielded_data2, [spend2]);

        block1.transactions[0] = transaction_v4_from_coinbase(&block1.transactions[0]).into();

        block1
            .transactions
            .extend([transaction1.into(), transaction2.into()]);

        let (finalized_state, mut non_finalized_state, genesis) = new_state_with_mainnet_genesis();

        // Allows anchor checks to pass
        finalized_state.populate_with_anchors(&block1);

        let previous_mem = non_finalized_state.clone();

        let block1 = Arc::new(block1).prepare();
        let commit_result = validate_and_commit_non_finalized(
            &finalized_state.db,
            &mut non_finalized_state,
            block1
        );

        prop_assert_eq!(
            commit_result,
            Err(DuplicateSaplingNullifier {
                nullifier: duplicate_nullifier,
                in_finalized_state: false,
            })
        );
        prop_assert_eq!(Some((Height(0), genesis.hash)), read::best_tip(&non_finalized_state, &finalized_state.db));
        prop_assert!(non_finalized_state.eq_internal_state(&previous_mem));
    }

    /// Make sure duplicate sapling nullifiers are rejected by state contextual validation,
    /// if they come from different blocks in the same chain.
    #[test]
    fn reject_duplicate_sapling_nullifiers_in_chain(
        spend1 in sapling::arbitrary::spend(),
        mut spend2 in sapling::arbitrary::spend(),
        sapling_shielded_data1 in sapling::arbitrary::bundle(false),
        sapling_shielded_data2 in sapling::arbitrary::bundle(false),
        duplicate_in_finalized_state in any::<bool>(),
    ) {
        let _init_guard = zebra_test::init();

        let mut block1 = zebra_test::vectors::BLOCK_MAINNET_1_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");
        let mut block2 = zebra_test::vectors::BLOCK_MAINNET_2_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");

        // create a double-spend across two blocks
        let duplicate_nullifier = sapling::Nullifier::from(spend1.nullifier().0);
        spend2 = sapling::arbitrary::with_nullifier(&spend2, *spend1.nullifier());

        let transaction1 =
            Arc::new(transaction_v4_with_sapling_shielded_data(sapling_shielded_data1, [spend1]));
        let transaction2 =
            transaction_v4_with_sapling_shielded_data(sapling_shielded_data2, [spend2]);

        block1.transactions[0] = transaction_v4_from_coinbase(&block1.transactions[0]).into();
        block2.transactions[0] = transaction_v4_from_coinbase(&block2.transactions[0]).into();

        block1.transactions.push(transaction1.clone());
        block2.transactions.push(transaction2.into());

        let (mut finalized_state, mut non_finalized_state, _genesis) = new_state_with_mainnet_genesis();

        // Allows anchor checks to pass
        finalized_state.populate_with_anchors(&block1);
        finalized_state.populate_with_anchors(&block2);

        let mut previous_mem = non_finalized_state.clone();

        // makes sure there are no spurious rejections that might hide bugs in `tx_no_duplicates_in_chain`
        let check_tx_no_duplicates_in_chain =
            tx_no_duplicates_in_chain(&finalized_state.db, non_finalized_state.best_chain(), &transaction1);
        prop_assert!(check_tx_no_duplicates_in_chain.is_ok());

        let block1_hash;
        // randomly choose to commit the next block to the finalized or non-finalized state
        if duplicate_in_finalized_state {
            let block1 = CheckpointVerifiedBlock::from(Arc::new(block1));
            let commit_result = finalized_state.commit_finalized_direct(block1.clone().into(),None,  "test");

            prop_assert_eq!(Some((Height(1), block1.hash)), read::best_tip(&non_finalized_state, &finalized_state.db));
            prop_assert!(commit_result.is_ok());
            prop_assert!(non_finalized_state.eq_internal_state(&previous_mem));
            prop_assert!(finalized_state.contains_sapling_nullifier(&duplicate_nullifier));

            block1_hash = block1.hash;
        } else {
            let block1 = Arc::new(block1).prepare();
            let commit_result = validate_and_commit_non_finalized(
                &finalized_state.db,
                &mut non_finalized_state,
                block1.clone()
            );

            prop_assert_eq!(commit_result, Ok(()));
            prop_assert_eq!(Some((Height(1), block1.hash)), read::best_tip(&non_finalized_state, &finalized_state.db));
            prop_assert!(!non_finalized_state.eq_internal_state(&previous_mem));
            prop_assert!(non_finalized_state

                .best_contains_sapling_nullifier(&duplicate_nullifier));

            block1_hash = block1.hash;
            previous_mem = non_finalized_state.clone();
        }

        let block2 = Arc::new(block2).prepare();
        let commit_result = validate_and_commit_non_finalized(
            &finalized_state.db,
            &mut non_finalized_state,
            block2
        );

        prop_assert_eq!(
            commit_result,
            Err(DuplicateSaplingNullifier {
                nullifier: duplicate_nullifier,
                in_finalized_state: duplicate_in_finalized_state,
            })
        );

        let check_tx_no_duplicates_in_chain =
            tx_no_duplicates_in_chain(&finalized_state.db, non_finalized_state.best_chain(), &transaction1);

        prop_assert_eq!(
            check_tx_no_duplicates_in_chain,
            Err(DuplicateSaplingNullifier {
                nullifier: duplicate_nullifier,
                in_finalized_state: duplicate_in_finalized_state,
            })
        );

        prop_assert_eq!(Some((Height(1), block1_hash)), read::best_tip(&non_finalized_state, &finalized_state.db));
        prop_assert!(non_finalized_state.eq_internal_state(&previous_mem));
    }

    // orchard

    /// Make sure an arbitrary orchard nullifier is accepted by state contextual validation.
    ///
    /// This test makes sure there are no spurious rejections that might hide bugs in the other tests.
    /// (And that the test infrastructure generally works.)
    #[test]
    fn accept_distinct_arbitrary_orchard_nullifiers_in_one_block(
        action in orchard::arbitrary::action(),
        orchard_shielded_data in orchard::arbitrary::bundle(Nu5),
        use_finalized_state in any::<bool>(),
    ) {
        let _init_guard = zebra_test::init();

        let mut block1 = zebra_test::vectors::BLOCK_MAINNET_1_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");

        let expected_nullifier = orchard::Nullifier::from(*action.nullifier());

        let transaction = transaction_v5_with_orchard_shielded_data(
            orchard_shielded_data,
            [action],
        );

        // convert the coinbase transaction to a version that the non-finalized state will accept
        block1.transactions[0] = transaction_v4_from_coinbase(&block1.transactions[0]).into();

        block1.transactions.push(transaction.into());

    let (mut finalized_state, mut non_finalized_state, _genesis) = new_state_with_mainnet_genesis();

        // Allows anchor checks to pass
        finalized_state.populate_with_anchors(&block1);

        let previous_mem = non_finalized_state.clone();

        // randomly choose to commit the block to the finalized or non-finalized state
        if use_finalized_state {
            let block1 = CheckpointVerifiedBlock::from(Arc::new(block1));
            let commit_result = finalized_state.commit_finalized_direct(block1.clone().into(), None, "test");

            prop_assert_eq!(Some((Height(1), block1.hash)), read::best_tip(&non_finalized_state, &finalized_state.db));
            prop_assert!(commit_result.is_ok());
            prop_assert!(non_finalized_state.eq_internal_state(&previous_mem));
            prop_assert!(finalized_state.contains_orchard_nullifier(&expected_nullifier));
        } else {
            let block1 = Arc::new(block1).prepare();
            let commit_result = validate_and_commit_non_finalized(
                &finalized_state.db,
                &mut non_finalized_state,
                block1.clone()
            );

            prop_assert_eq!(commit_result, Ok(()));
            prop_assert_eq!(Some((Height(1), block1.hash)), read::best_tip(&non_finalized_state, &finalized_state.db));
            prop_assert!(!non_finalized_state.eq_internal_state(&previous_mem));
            prop_assert!(non_finalized_state

                .best_contains_orchard_nullifier(&expected_nullifier));
        }
    }

    /// Make sure duplicate orchard nullifiers are rejected by state contextual validation,
    /// if they come from different AuthorizedActions in the same orchard::ShieldedData/Transaction.
    #[test]
    fn reject_duplicate_orchard_nullifiers_in_transaction(
        action1 in orchard::arbitrary::action(),
        mut action2 in orchard::arbitrary::action(),
        orchard_shielded_data in orchard::arbitrary::bundle(Nu5),
    ) {
        let _init_guard = zebra_test::init();

        let mut block1 = zebra_test::vectors::BLOCK_MAINNET_1_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");

        // create a double-spend across two actions
        let duplicate_nullifier = orchard::Nullifier::from(*action1.nullifier());
        action2 = orchard::arbitrary::with_nullifier(&action2, *action1.nullifier());

        let transaction = transaction_v5_with_orchard_shielded_data(
            orchard_shielded_data,
            [action1, action2],
        );

        block1.transactions[0] = transaction_v4_from_coinbase(&block1.transactions[0]).into();

        block1.transactions.push(transaction.into());

            let (finalized_state, mut non_finalized_state, genesis) = new_state_with_mainnet_genesis();

        // Allows anchor checks to pass
        finalized_state.populate_with_anchors(&block1);

        let previous_mem = non_finalized_state.clone();

        let block1 = Arc::new(block1).prepare();
        let commit_result = validate_and_commit_non_finalized(
            &finalized_state.db,
            &mut non_finalized_state,
            block1
        );

        prop_assert_eq!(
            commit_result,
            Err(DuplicateOrchardNullifier {
                nullifier: duplicate_nullifier,
                in_finalized_state: false,
            })
        );
        prop_assert_eq!(Some((Height(0), genesis.hash)), read::best_tip(&non_finalized_state, &finalized_state.db));
        prop_assert!(non_finalized_state.eq_internal_state(&previous_mem));
    }

    /// Make sure duplicate orchard nullifiers are rejected by state contextual validation,
    /// if they come from different transactions in the same block.
    #[test]
    fn reject_duplicate_orchard_nullifiers_in_block(
        action1 in orchard::arbitrary::action(),
        mut action2 in orchard::arbitrary::action(),
        orchard_shielded_data1 in orchard::arbitrary::bundle(Nu5),
        orchard_shielded_data2 in orchard::arbitrary::bundle(Nu5),
    ) {
        let _init_guard = zebra_test::init();

        let mut block1 = zebra_test::vectors::BLOCK_MAINNET_1_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");

        // create a double-spend across two transactions
        let duplicate_nullifier = orchard::Nullifier::from(*action1.nullifier());
        action2 = orchard::arbitrary::with_nullifier(&action2, *action1.nullifier());

        let transaction1 = transaction_v5_with_orchard_shielded_data(
            orchard_shielded_data1,
            [action1],
        );
        let transaction2 = transaction_v5_with_orchard_shielded_data(
            orchard_shielded_data2,
            [action2],
        );

        block1.transactions[0] = transaction_v4_from_coinbase(&block1.transactions[0]).into();

        block1
            .transactions
            .extend([transaction1.into(), transaction2.into()]);

            let (finalized_state, mut non_finalized_state, genesis) = new_state_with_mainnet_genesis();

        // Allows anchor checks to pass
        finalized_state.populate_with_anchors(&block1);

        let previous_mem = non_finalized_state.clone();

        let block1 = Arc::new(block1).prepare();
        let commit_result = validate_and_commit_non_finalized(
            &finalized_state.db,
            &mut non_finalized_state,
            block1
        );

        prop_assert_eq!(
            commit_result,
            Err(DuplicateOrchardNullifier {
                nullifier: duplicate_nullifier,
                in_finalized_state: false,
            })
        );
        prop_assert_eq!(Some((Height(0), genesis.hash)), read::best_tip(&non_finalized_state, &finalized_state.db));
        prop_assert!(non_finalized_state.eq_internal_state(&previous_mem));
    }

    /// Make sure duplicate orchard nullifiers are rejected by state contextual validation,
    /// if they come from different blocks in the same chain.
    #[test]
    fn reject_duplicate_orchard_nullifiers_in_chain(
        action1 in orchard::arbitrary::action(),
        mut action2 in orchard::arbitrary::action(),
        orchard_shielded_data1 in orchard::arbitrary::bundle(Nu5),
        orchard_shielded_data2 in orchard::arbitrary::bundle(Nu5),
        duplicate_in_finalized_state in any::<bool>(),
    ) {
        let _init_guard = zebra_test::init();

        let mut block1 = zebra_test::vectors::BLOCK_MAINNET_1_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");
        let mut block2 = zebra_test::vectors::BLOCK_MAINNET_2_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");

        // create a double-spend across two blocks
        let duplicate_nullifier = orchard::Nullifier::from(*action1.nullifier());
        action2 = orchard::arbitrary::with_nullifier(&action2, *action1.nullifier());

        let transaction1 = Arc::new(transaction_v5_with_orchard_shielded_data(
            orchard_shielded_data1,
            [action1],
        ));
        let transaction2 = transaction_v5_with_orchard_shielded_data(
            orchard_shielded_data2,
            [action2],
        );

        block1.transactions[0] = transaction_v4_from_coinbase(&block1.transactions[0]).into();
        block2.transactions[0] = transaction_v4_from_coinbase(&block2.transactions[0]).into();

        block1.transactions.push(transaction1.clone());
        block2.transactions.push(transaction2.into());

    let (mut finalized_state, mut non_finalized_state, _genesis) = new_state_with_mainnet_genesis();

        // Allows anchor checks to pass
        finalized_state.populate_with_anchors(&block1);
        finalized_state.populate_with_anchors(&block2);

        let mut previous_mem = non_finalized_state.clone();

        // makes sure there are no spurious rejections that might hide bugs in `tx_no_duplicates_in_chain`
        let check_tx_no_duplicates_in_chain =
            tx_no_duplicates_in_chain(&finalized_state.db, non_finalized_state.best_chain(), &transaction1);
        prop_assert!(check_tx_no_duplicates_in_chain.is_ok());

        let block1_hash;
        // randomly choose to commit the next block to the finalized or non-finalized state
        if duplicate_in_finalized_state {
            let block1 = CheckpointVerifiedBlock::from(Arc::new(block1));
            let commit_result = finalized_state.commit_finalized_direct(block1.clone().into(), None, "test");

            prop_assert_eq!(Some((Height(1), block1.hash)), read::best_tip(&non_finalized_state, &finalized_state.db));
            prop_assert!(commit_result.is_ok());
            prop_assert!(non_finalized_state.eq_internal_state(&previous_mem));
            prop_assert!(finalized_state.contains_orchard_nullifier(&duplicate_nullifier));

            block1_hash = block1.hash;
        } else {
            let block1 = Arc::new(block1).prepare();
            let commit_result = validate_and_commit_non_finalized(
                &finalized_state.db,
                &mut non_finalized_state,
                block1.clone()
            );

            prop_assert_eq!(commit_result, Ok(()));
            prop_assert_eq!(Some((Height(1), block1.hash)), read::best_tip(&non_finalized_state, &finalized_state.db));
            prop_assert!(!non_finalized_state.eq_internal_state(&previous_mem));
            prop_assert!(non_finalized_state
                .best_contains_orchard_nullifier(&duplicate_nullifier));

            block1_hash = block1.hash;
            previous_mem = non_finalized_state.clone();
        }

        let block2 = Arc::new(block2).prepare();
        let commit_result = validate_and_commit_non_finalized(
            &finalized_state.db,
            &mut non_finalized_state,
            block2
        );

        prop_assert_eq!(
            commit_result,
            Err(DuplicateOrchardNullifier {
                nullifier: duplicate_nullifier,
                in_finalized_state: duplicate_in_finalized_state,
            })
        );

        let check_tx_no_duplicates_in_chain =
            tx_no_duplicates_in_chain(&finalized_state.db, non_finalized_state.best_chain(), &transaction1);

        prop_assert_eq!(
            check_tx_no_duplicates_in_chain,
            Err(DuplicateOrchardNullifier {
                nullifier: duplicate_nullifier,
                in_finalized_state: duplicate_in_finalized_state,
            })
        );

        prop_assert_eq!(Some((Height(1), block1_hash)), read::best_tip(&non_finalized_state, &finalized_state.db));
        prop_assert!(non_finalized_state.eq_internal_state(&previous_mem));
    }

    /// A block whose sprout-shielded transaction has the same hash as one
    /// already finalized must be rejected with `DuplicateSproutNullifier`.
    #[test]
    fn reject_block_containing_sprout_tx_already_in_finalized_chain(
        joinsplit in sprout::arbitrary::joinsplit(true),
        joinsplit_data in sprout::arbitrary::joinsplit_data(true),
    ) {
        let _init_guard = zebra_test::init();

        let mut block1 = zebra_test::vectors::BLOCK_MAINNET_1_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");
        let mut block2 = zebra_test::vectors::BLOCK_MAINNET_2_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");

        let mut nullifiers = *joinsplit.nullifiers();
        make_distinct_nullifiers(&mut nullifiers);
        let joinsplit = sprout::arbitrary::with_nullifiers(&joinsplit, nullifiers);
        let expected_duplicate_nullifier = sprout::Nullifier::from(nullifiers[0]);

        let transaction = Arc::new(transaction_v4_with_joinsplit_data(joinsplit_data, [joinsplit]));

        block1.transactions[0] = transaction_v4_from_coinbase(&block1.transactions[0]).into();
        block2.transactions[0] = transaction_v4_from_coinbase(&block2.transactions[0]).into();

        // Push the same Arc into both blocks so they share a tx hash.
        block1.transactions.push(transaction.clone());
        block2.transactions.push(transaction);

        let (mut finalized_state, mut non_finalized_state, _genesis) = new_state_with_mainnet_genesis();

        finalized_state.populate_with_anchors(&block1);
        finalized_state.populate_with_anchors(&block2);

        let block1 = CheckpointVerifiedBlock::from(Arc::new(block1));
        let commit_result = finalized_state.commit_finalized_direct(block1.into(), None, "test");
        prop_assert!(commit_result.is_ok());

        let block2 = Arc::new(block2).prepare();
        let commit_result = validate_and_commit_non_finalized(
            &finalized_state.db,
            &mut non_finalized_state,
            block2,
        );

        prop_assert_eq!(
            commit_result,
            Err(DuplicateSproutNullifier {
                nullifier: expected_duplicate_nullifier,
                in_finalized_state: true,
            })
        );
    }

    /// A block whose sapling-shielded transaction has the same hash as one
    /// already finalized must be rejected with `DuplicateSaplingNullifier`.
    #[test]
    fn reject_block_containing_sapling_tx_already_in_finalized_chain(
        spend in sapling::arbitrary::spend(),
        sapling_shielded_data in sapling::arbitrary::bundle(false),
    ) {
        let _init_guard = zebra_test::init();

        let mut block1 = zebra_test::vectors::BLOCK_MAINNET_1_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");
        let mut block2 = zebra_test::vectors::BLOCK_MAINNET_2_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");

        let expected_duplicate_nullifier = sapling::Nullifier::from(spend.nullifier().0);

        let transaction = Arc::new(transaction_v4_with_sapling_shielded_data(
            sapling_shielded_data,
            [spend],
        ));

        block1.transactions[0] = transaction_v4_from_coinbase(&block1.transactions[0]).into();
        block2.transactions[0] = transaction_v4_from_coinbase(&block2.transactions[0]).into();

        block1.transactions.push(transaction.clone());
        block2.transactions.push(transaction);

        let (mut finalized_state, mut non_finalized_state, _genesis) = new_state_with_mainnet_genesis();

        finalized_state.populate_with_anchors(&block1);
        finalized_state.populate_with_anchors(&block2);

        let block1 = CheckpointVerifiedBlock::from(Arc::new(block1));
        let commit_result = finalized_state.commit_finalized_direct(block1.into(), None, "test");
        prop_assert!(commit_result.is_ok());

        let block2 = Arc::new(block2).prepare();
        let commit_result = validate_and_commit_non_finalized(
            &finalized_state.db,
            &mut non_finalized_state,
            block2,
        );

        prop_assert_eq!(
            commit_result,
            Err(DuplicateSaplingNullifier {
                nullifier: expected_duplicate_nullifier,
                in_finalized_state: true,
            })
        );
    }

    /// A block whose orchard-shielded transaction has the same hash as one
    /// already finalized must be rejected with `DuplicateOrchardNullifier`.
    #[test]
    fn reject_block_containing_orchard_tx_already_in_finalized_chain(
        action in orchard::arbitrary::action(),
        orchard_shielded_data in orchard::arbitrary::bundle(Nu5),
    ) {
        let _init_guard = zebra_test::init();

        let mut block1 = zebra_test::vectors::BLOCK_MAINNET_1_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");
        let mut block2 = zebra_test::vectors::BLOCK_MAINNET_2_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");

        let expected_duplicate_nullifier = orchard::Nullifier::from(*action.nullifier());

        let transaction = Arc::new(transaction_v5_with_orchard_shielded_data(
            orchard_shielded_data,
            [action],
        ));

        block1.transactions[0] = transaction_v4_from_coinbase(&block1.transactions[0]).into();
        block2.transactions[0] = transaction_v4_from_coinbase(&block2.transactions[0]).into();

        block1.transactions.push(transaction.clone());
        block2.transactions.push(transaction);

        let (mut finalized_state, mut non_finalized_state, _genesis) = new_state_with_mainnet_genesis();

        finalized_state.populate_with_anchors(&block1);
        finalized_state.populate_with_anchors(&block2);

        let block1 = CheckpointVerifiedBlock::from(Arc::new(block1));
        let commit_result = finalized_state.commit_finalized_direct(block1.into(), None, "test");
        prop_assert!(commit_result.is_ok());

        let block2 = Arc::new(block2).prepare();
        let commit_result = validate_and_commit_non_finalized(
            &finalized_state.db,
            &mut non_finalized_state,
            block2,
        );

        prop_assert_eq!(
            commit_result,
            Err(DuplicateOrchardNullifier {
                nullifier: expected_duplicate_nullifier,
                in_finalized_state: true,
            })
        );
    }

    /// A block whose sapling-shielded transaction has the same hash as one
    /// already present in the **non-finalized** part of the chain must be
    /// rejected with `DuplicateSaplingNullifier` — not panic.
    ///
    /// This is the BIP30-style duplicate-txid scenario where the prior
    /// occurrence lives in the non-finalized chain (so the
    /// `no_duplicates_in_finalized_chain` check in `initial_contextual_validity`
    /// cannot catch it). The fix in
    /// `zebra-state/src/service/non_finalized_state/chain.rs` runs the
    /// shielded nullifier check before the `tx_loc_by_hash` assertion,
    /// so the duplicate is rejected via the existing nullifier check.
    #[test]
    fn reject_block_containing_sapling_tx_already_in_non_finalized_chain(
        spend in sapling::arbitrary::spend(),
        sapling_shielded_data in sapling::arbitrary::bundle(false),
    ) {
        let _init_guard = zebra_test::init();

        let mut block1 = zebra_test::vectors::BLOCK_MAINNET_1_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");
        let mut block2 = zebra_test::vectors::BLOCK_MAINNET_2_BYTES
            .zcash_deserialize_into::<Block>()
            .expect("block should deserialize");

        let expected_duplicate_nullifier = sapling::Nullifier::from(spend.nullifier().0);

        let transaction = Arc::new(transaction_v4_with_sapling_shielded_data(
            sapling_shielded_data,
            [spend],
        ));

        block1.transactions[0] = transaction_v4_from_coinbase(&block1.transactions[0]).into();
        block2.transactions[0] = transaction_v4_from_coinbase(&block2.transactions[0]).into();

        // Push the same Arc into both blocks so they share a tx hash.
        block1.transactions.push(transaction.clone());
        block2.transactions.push(transaction);

        let (finalized_state, mut non_finalized_state, _genesis) = new_state_with_mainnet_genesis();

        finalized_state.populate_with_anchors(&block1);
        finalized_state.populate_with_anchors(&block2);

        // Commit block1 to the *non-finalized* state so the duplicate-tx
        // scenario is purely within the non-finalized chain.
        let block1 = Arc::new(block1).prepare();
        let commit_result = validate_and_commit_non_finalized(
            &finalized_state.db,
            &mut non_finalized_state,
            block1,
        );
        prop_assert_eq!(commit_result, Ok(()));

        let block2 = Arc::new(block2).prepare();
        let commit_result = validate_and_commit_non_finalized(
            &finalized_state.db,
            &mut non_finalized_state,
            block2,
        );

        prop_assert_eq!(
            commit_result,
            Err(DuplicateSaplingNullifier {
                nullifier: expected_duplicate_nullifier,
                in_finalized_state: false,
            })
        );
    }
}

/// Make sure the supplied nullifiers are distinct, modifying them if necessary.
fn make_distinct_nullifiers<'until_modified, NullifierT>(
    nullifiers: impl IntoIterator<Item = &'until_modified mut NullifierT>,
) where
    NullifierT: Into<[u8; 32]> + Clone + Eq + std::hash::Hash + 'until_modified,
    [u8; 32]: Into<NullifierT>,
{
    let nullifiers: Vec<_> = nullifiers.into_iter().collect();

    if nullifiers.iter().unique().count() < nullifiers.len() {
        let mut tweak: u8 = 0x00;
        for nullifier in nullifiers {
            let mut nullifier_bytes: [u8; 32] = nullifier.clone().into();
            nullifier_bytes[0] = tweak;
            *nullifier = nullifier_bytes.into();

            tweak = tweak
                .checked_add(0x01)
                .expect("unexpectedly large nullifier list");
        }
    }
}

/// Return a V4 transaction containing `joinsplit_data` with its JoinSplits replaced by
/// `joinsplits`, and zero public values.
///
/// Other fields have empty or default values.
fn transaction_v4_with_joinsplit_data(
    joinsplit_data: sprout::JoinSplitData,
    joinsplits: impl IntoIterator<Item = sprout::JoinSplit>,
) -> CompressedTransaction {
    // zero public values, so the chain value pool checks pass
    let joinsplits = joinsplits
        .into_iter()
        .map(|joinsplit| sprout::arbitrary::with_values(&joinsplit, Amount::zero(), Amount::zero()))
        .collect();

    CompressedTransaction::test_v4_with_sprout(Some(sprout::JoinSplitData {
        joinsplits,
        ..joinsplit_data
    }))
}

/// Return a V4 transaction containing `sapling_shielded_data`'s outputs and `spends`, with a
/// zero value balance.
///
/// Other fields have empty or default values.
///
/// Note: since sapling nullifiers in V5 transactions are identical to V4 transactions,
/// we just use V4 transactions in the tests.
///
/// # Panics
///
/// If there are no `Spend`s in `spends`, and no `Output`s in `sapling_shielded_data`.
fn transaction_v4_with_sapling_shielded_data(
    sapling_shielded_data: sapling::arbitrary::Bundle,
    spends: impl IntoIterator<Item = sapling::arbitrary::Spend>,
) -> CompressedTransaction {
    CompressedTransaction::test_v4_with_sapling(
        Vec::new(),
        Vec::new(),
        LockTime::min_lock_time_timestamp(),
        Height(0),
        Some(
            sapling::arbitrary::with_spends(&sapling_shielded_data, spends)
                .expect("a spend or an output"),
        ),
    )
}

/// Return a NU5 V5 transaction containing `orchard_shielded_data` with its actions replaced by
/// `actions`, and a zero value balance.
///
/// Other fields have empty or default values.
///
/// # Panics
///
/// If there are no `Action`s in `actions`.
fn transaction_v5_with_orchard_shielded_data(
    orchard_shielded_data: orchard::arbitrary::Bundle,
    actions: impl IntoIterator<Item = orchard::arbitrary::Action>,
) -> CompressedTransaction {
    CompressedTransaction::test_v5_with_orchard(
        Nu5,
        Vec::new(),
        Vec::new(),
        LockTime::min_lock_time_timestamp(),
        Height(0),
        Some(orchard::arbitrary::with_actions(
            &orchard_shielded_data,
            actions,
        )),
    )
}
