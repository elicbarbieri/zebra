//! Fixed test vectors for zebra-network configuration.

use std::num::NonZeroUsize;

use static_assertions::const_assert;
use zebra_chain::{
    block::Height,
    parameters::{
        testnet::{self, ConfiguredFundingStreams},
        Network,
    },
};

use crate::{
    constants::{INBOUND_PEER_LIMIT_MULTIPLIER, OUTBOUND_PEER_LIMIT_MULTIPLIER},
    Config, ReservedListener,
};

#[test]
fn parse_config_listen_addr() {
    let _init_guard = zebra_test::init();

    let fixtures = vec![
        ("listen_addr = '0.0.0.0'", "0.0.0.0:8233"),
        ("listen_addr = '0.0.0.0:9999'", "0.0.0.0:9999"),
        (
            "listen_addr = '0.0.0.0'\nnetwork = 'Testnet'",
            "0.0.0.0:18233",
        ),
        (
            "listen_addr = '0.0.0.0:8233'\nnetwork = 'Testnet'",
            "0.0.0.0:8233",
        ),
        ("listen_addr = '[::]'", "[::]:8233"),
        ("listen_addr = '[::]:9999'", "[::]:9999"),
        ("listen_addr = '[::]'\nnetwork = 'Testnet'", "[::]:18233"),
        (
            "listen_addr = '[::]:8233'\nnetwork = 'Testnet'",
            "[::]:8233",
        ),
        ("listen_addr = '[::1]:8233'", "[::1]:8233"),
        ("listen_addr = '[2001:db8::1]:8233'", "[2001:db8::1]:8233"),
    ];

    for (config, value) in fixtures {
        let config: Config = toml::from_str(config).unwrap();
        assert_eq!(config.listen_addr.to_string(), value);
    }
}

/// Make sure the peer connection limits are consistent with each other.
#[test]
fn ensure_peer_connection_limits_consistent() {
    let _init_guard = zebra_test::init();

    // Zebra should allow more inbound connections, to avoid connection exhaustion
    const_assert!(INBOUND_PEER_LIMIT_MULTIPLIER > OUTBOUND_PEER_LIMIT_MULTIPLIER);

    let config = Config::default();

    assert!(
        config.peerset_inbound_connection_limit() - config.peerset_outbound_connection_limit()
            >= 50,
        "default config should allow more inbound connections, to avoid connection exhaustion",
    );
}

#[test]
fn testnet_params_serialization_roundtrip() {
    let _init_guard = zebra_test::init();

    let config = Config {
        network: testnet::Parameters::build()
            .with_disable_pow(true)
            .to_network()
            .expect("failed to build configured network"),
        initial_testnet_peers: [].into(),
        ..Config::default()
    };

    let serialized = toml::to_string(&config).unwrap();
    let deserialized: Config = toml::from_str(&serialized).unwrap();

    assert_eq!(config, deserialized);
}

#[test]
fn default_config_uses_ipv6() {
    let _init_guard = zebra_test::init();
    let config = Config::default();

    assert_eq!(config.listen_addr.to_string(), "[::]:8233");
    assert!(config.listen_addr.is_ipv6());
}

#[test]
fn funding_streams_serialization_roundtrip() {
    let _init_guard = zebra_test::init();

    let fs = testnet::Parameters::default()
        .funding_streams()
        .iter()
        .map(ConfiguredFundingStreams::from)
        .collect();

    let config = Config {
        network: testnet::Parameters::build()
            .with_funding_streams(fs)
            .to_network()
            .expect("failed to build configured network"),
        initial_testnet_peers: [].into(),
        ..Config::default()
    };

    let serialized = toml::to_string(&config).unwrap();
    let deserialized: Config = toml::from_str(&serialized).unwrap();

    assert_eq!(config, deserialized);
}

/// Checks that a configured Testnet's temporary Orchard-disabling soft fork height
/// survives a serialization round-trip.
#[test]
fn temporary_orchard_disabling_soft_fork_height_serialization_roundtrip() {
    let _init_guard = zebra_test::init();

    let soft_fork_height = Height(2_000_000);

    let config = Config {
        network: testnet::Parameters::build()
            .with_temporary_orchard_disabling_soft_fork_height(soft_fork_height)
            .to_network()
            .expect("failed to build configured network"),
        initial_testnet_peers: [].into(),
        ..Config::default()
    };

    let serialized = toml::to_string(&config).unwrap();
    let deserialized: Config = toml::from_str(&serialized).unwrap();

    assert_eq!(config, deserialized);

    // The configured height must be preserved through the round-trip.
    let Network::Testnet(params) = &deserialized.network else {
        panic!("deserialized network must be a Testnet");
    };
    assert_eq!(
        params.temporary_orchard_disabling_soft_fork_height(),
        Some(soft_fork_height),
    );
}

/// Checks that a Regtest configured to forbid unshielded coinbase spends survives a
/// serialization round-trip, and that the flag does not change the network's identity.
#[test]
fn regtest_should_allow_unshielded_coinbase_spends_serialization_roundtrip() {
    let _init_guard = zebra_test::init();

    let config = Config {
        network: Network::new_regtest(testnet::RegtestParameters {
            should_allow_unshielded_coinbase_spends: Some(false),
            ..Default::default()
        }),
        initial_testnet_peers: [].into(),
        ..Config::default()
    };

    // Forbidding unshielded coinbase spends must not stop the network from being Regtest.
    assert!(config.network.is_regtest());

    let serialized = toml::to_string(&config).unwrap();
    let deserialized: Config = toml::from_str(&serialized).unwrap();

    assert_eq!(config, deserialized);
    assert!(deserialized.network.is_regtest());

    let Network::Testnet(params) = &deserialized.network else {
        panic!("deserialized network must be Regtest");
    };
    assert!(!params.should_allow_unshielded_coinbase_spends());
}

/// Checks that the Regtest-only `should_allow_unshielded_coinbase_spends` knob is rejected
/// on a configured Testnet rather than silently ignored.
#[test]
fn should_allow_unshielded_coinbase_spends_rejected_on_testnet() {
    let _init_guard = zebra_test::init();

    let toml = "network = 'Testnet'\n\n[testnet_parameters]\nshould_allow_unshielded_coinbase_spends = true\n";
    let err = toml::from_str::<Config>(toml)
        .expect_err("configured Testnet must reject the Regtest-only field");

    assert!(
        err.to_string()
            .contains("should_allow_unshielded_coinbase_spends"),
        "unexpected error: {err}"
    );
}

#[test]
fn reserved_listener_parses_validates_and_widens_the_total_limit() {
    let _init_guard = zebra_test::init();

    let default_config = Config::default();
    assert_eq!(default_config.reserved_inbound_connection_limit(), 0);
    assert!(
        !toml::to_string(&default_config)
            .unwrap()
            .contains("reserved_listener"),
        "an unset reserved listener must stay out of generated configs"
    );

    // public inbound limit = 2 * INBOUND_PEER_LIMIT_MULTIPLIER = 10 = the largest accepted slots
    let config: Config = toml::from_str(
        "peerset_initial_target_size = 2

         [reserved_listener]
         listen_addr = '[::ffff:127.0.0.1]:8234'
         slots = 10",
    )
    .expect("slots equal to the public inbound limit are valid");

    assert_eq!(
        config.reserved_listener,
        Some(ReservedListener {
            listen_addr: "127.0.0.1:8234".parse().unwrap(),
            slots: NonZeroUsize::new(10).unwrap(),
        }),
        "IPv4-mapped listen addresses are canonicalized like `listen_addr`"
    );
    assert_eq!(
        config.peerset_total_connection_limit(),
        config.peerset_outbound_connection_limit() + config.peerset_inbound_connection_limit() + 10,
    );
    assert_eq!(
        toml::from_str::<Config>(&toml::to_string(&config).unwrap()).unwrap(),
        config
    );

    // default peerset_initial_target_size = 25 → public inbound limit = 125
    let rejected = [
        ("listen_addr = '[::]:8234'\nslots = 0", "nonzero"),
        (
            "listen_addr = '[::]:8234'\nslots = 126",
            "slots = 126 exceeds the public inbound limit of 125",
        ),
        ("listen_addr = '[::]'\nslots = 1", "invalid socket address"),
        ("slots = 1", "missing field `listen_addr`"),
        ("listen_addr = '[::]:8234'", "missing field `slots`"),
        (
            "listen_addr = '[::]:8234'\nslots = 1\nnetworks = []",
            "unknown field `networks`",
        ),
    ];
    for (listener, expected) in rejected {
        let toml = format!("[reserved_listener]\n{listener}");
        let err = toml::from_str::<Config>(&toml).expect_err(&toml);

        assert!(
            err.to_string().contains(expected),
            "config:\n{toml}\nexpected error containing `{expected}`, got: {err}"
        );
    }
}
