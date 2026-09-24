//! End-to-end `getblock` latency and throughput, as an RPC consumer sees it.
//!
//! - Real `RpcServer` on a socket (usual middleware), HTTP client reading every body in full
//! - Server + wire only (consumer-side parsing = identical either way, dilutes the comparison)
//! - `getblock` reads stored transactions every call, verifies nothing (no point to decompress)
//!
//! State opened read-only (shareable with a running node). `ZEBRA_CACHED_STATE_DIR` unset → usage
//! printed, exits.
//!
//! ```text
//! ZEBRA_CACHED_STATE_DIR=/path/to/state cargo bench -p zebra-rpc --bench getblock
//! ```
//!
//! | variable | default | meaning |
//! |---|---|---|
//! | `ZEBRA_CACHED_STATE_DIR` | — | state directory, opened read-only |
//! | `ZEBRA_BENCH_NETWORK` | `mainnet` | `mainnet` or `testnet` |
//! | `ZEBRA_BENCH_VERBOSITY` | `0,1,2` | `getblock` verbosity levels to measure |
//! | `ZEBRA_BENCH_CONCURRENCY` | `1,2,4,8,16,32` | in-flight requests per measurement |
//! | `ZEBRA_BENCH_SECONDS` | `5` | measurement window per (window, concurrency) |
//! | `ZEBRA_BENCH_WINDOWS` | last 10000 | `name=lo..hi` height ranges, comma-separated |
//! | `ZEBRA_BENCH_SELECT` | `uniform` | `uniform`, or `shielded` for the densest blocks per window |
//! | `ZEBRA_BENCH_PORT` | `28232` | loopback port for the benchmark's own server |
//!
//! Windows = unit of comparison, composition reported per window (mostly transparent window =
//! RocksDB + JSON rendering, not parsing). One state serves all:
//!
//! ```text
//! ZEBRA_BENCH_WINDOWS=pre=1687104..1704322,spam=1704323..1760000,modern=3400000..3434143
//! ```

// Measurements → stdout (like `zebra-checkpoints`)
#![allow(clippy::print_stdout)]

use std::{
    env,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use tower::buffer::Buffer;

use zebra_chain::{
    block::{Block, Height},
    chain_sync_status::MockSyncStatus,
    chain_tip::mock::MockChainTip,
    parameters::Network::{self, *},
    transaction::TransactionExt,
};
use zebra_network::address_book_peers::MockAddressBookPeers;
use zebra_node_services::rpc_client::RpcRequestClient;
use zebra_rpc::{methods::RpcImpl, server::RpcServer};
use zebra_test::mock_service::MockService;

/// One measurement: a height window driven at a fixed number of in-flight requests.
struct Measurement {
    concurrency: usize,
    requests: u64,
    errors: u64,
    bytes: u64,
    elapsed: Duration,
    /// Microseconds, every worker
    latencies: Vec<u64>,
}

impl Measurement {
    fn per_second(&self) -> f64 {
        self.requests as f64 / self.elapsed.as_secs_f64()
    }

    /// Response bytes/s (= an indexer's sync rate)
    fn mib_per_second(&self) -> f64 {
        self.bytes as f64 / self.elapsed.as_secs_f64() / (1024.0 * 1024.0)
    }

    fn quantile(&self, q: f64) -> f64 {
        if self.latencies.is_empty() {
            return f64::NAN;
        }

        let mut sorted = self.latencies.clone();
        sorted.sort_unstable();
        let index = ((sorted.len() - 1) as f64 * q).round() as usize;
        sorted[index] as f64 / 1000.0
    }
}

/// Heights a worker requests: fixed pool (same work per measurement), LCG seeded per worker
/// (deterministic → runs comparable)
struct Heights {
    pool: Arc<Vec<u32>>,
    state: u64,
}

impl Heights {
    fn new(pool: Arc<Vec<u32>>, worker: usize) -> Self {
        Self {
            pool,
            state: 0x2545_F491_4F6C_DD1D ^ (worker as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15),
        }
    }

    fn next(&mut self) -> u32 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.pool[(self.state >> 33) as usize % self.pool.len()]
    }
}

/// Sapling spends & outputs, Orchard & Ironwood actions, Sprout JoinSplits (descriptions with
/// points to decompress)
fn shielded_descriptions(block: &Block) -> usize {
    block
        .transactions
        .iter()
        .map(|tx| {
            tx.sapling_spends_count()
                + tx.sapling_outputs().count()
                + tx.orchard_actions().count()
                + tx.ironwood_actions().count()
                + tx.joinsplit_count()
        })
        .sum()
}

/// Sampled blocks' contents (decides how much of a request is transaction parsing)
#[derive(Default)]
struct Composition {
    blocks: usize,
    transactions: usize,
    shielded: usize,
    bytes: usize,
}

impl Composition {
    fn of(db: &zebra_state::ZebraDb, heights: &[u32]) -> Self {
        let mut totals = Composition::default();

        for height in heights {
            let Some((block, size)) = db.block_and_size(Height(*height).into()) else {
                continue;
            };

            totals.blocks += 1;
            totals.bytes += size;
            totals.transactions += block.transactions.len();
            totals.shielded += shielded_descriptions(&block);
        }

        totals
    }

    fn report(&self, label: &str) {
        let blocks = self.blocks.max(1) as f64;
        println!(
            "{label}: {} blocks, {:.1} txs/block, {:.1} shielded descriptions/block, {:.1} KiB/block",
            self.blocks,
            self.transactions as f64 / blocks,
            self.shielded as f64 / blocks,
            self.bytes as f64 / blocks / 1024.0,
        );
    }
}

/// Named height range to measure
///
/// - Shielded density varies by orders of magnitude (pre-spam NU5 ≈ 1 description/block, spam
///   era = hundreds) → a `getblock` figure needs its chain range
struct Window {
    name: String,
    lowest: u32,
    highest: u32,
}

impl Window {
    /// `name=lo..hi` pairs, comma-separated
    fn parse(spec: &str, tip: Height) -> Vec<Self> {
        spec.split(',')
            .filter(|entry| !entry.trim().is_empty())
            .map(|entry| {
                let (name, range) = entry
                    .split_once('=')
                    .unwrap_or_else(|| panic!("window {entry:?} should be `name=lo..hi`"));
                let (lowest, highest) = range
                    .split_once("..")
                    .unwrap_or_else(|| panic!("window {entry:?} should be `name=lo..hi`"));

                let parse = |value: &str, field| {
                    value
                        .trim()
                        .parse::<u32>()
                        .unwrap_or_else(|_| panic!("window {entry:?} {field} should be a height"))
                };

                Self {
                    name: name.trim().to_string(),
                    lowest: parse(lowest, "start"),
                    highest: parse(highest, "end").min(tip.0),
                }
            })
            .collect()
    }
}

/// Blocks per window, capped to stay in the page cache (mainnet state ≫ RAM; uncapped = NVMe
/// seeks) yet not one outlier block
const POOL_BLOCKS: usize = 256;

/// Heights to measure within `window`
///
/// - `uniform`: strided (arbitrary requests)
/// - `shielded`: densest blocks (only place parsing costs anything)
fn height_pool(db: &zebra_state::ZebraDb, window: &Window, select: &str) -> Vec<u32> {
    let candidates: Vec<u32> = (window.lowest..=window.highest).collect();

    if select != "shielded" {
        let stride = candidates.len().div_ceil(POOL_BLOCKS);
        return candidates.into_iter().step_by(stride.max(1)).collect();
    }

    let mut ranked: Vec<(usize, u32)> = candidates
        .iter()
        .filter_map(|height| {
            let block = db.block(Height(*height).into())?;
            Some((shielded_descriptions(&block), *height))
        })
        .collect();

    ranked.sort_unstable_by_key(|(shielded, _)| std::cmp::Reverse(*shielded));
    ranked.truncate(POOL_BLOCKS);
    ranked.into_iter().map(|(_, height)| height).collect()
}

fn env_list(name: &str, default: &str) -> Vec<u64> {
    env::var(name)
        .unwrap_or_else(|_| default.to_string())
        .split(',')
        .filter_map(|value| value.trim().parse().ok())
        .collect()
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let Ok(cache_dir) = env::var("ZEBRA_CACHED_STATE_DIR") else {
        println!(
            "ZEBRA_CACHED_STATE_DIR is unset, so there is no chain to serve; skipping.\n\
             \n\
             Point it at a Zebra state directory, which is opened read-only:\n\
             \x20   ZEBRA_CACHED_STATE_DIR=/path/to/state \\\n\
             \x20     cargo bench -p zebra-rpc --bench getblock"
        );
        return;
    };

    let network = match env::var("ZEBRA_BENCH_NETWORK").as_deref() {
        Ok("testnet") => Network::new_default_testnet(),
        _ => Mainnet,
    };
    let verbosities: Vec<u8> = env_list("ZEBRA_BENCH_VERBOSITY", "0,1,2")
        .into_iter()
        .map(|v| u8::try_from(v).expect("ZEBRA_BENCH_VERBOSITY should list 0, 1 or 2"))
        .collect();
    let concurrencies = env_list("ZEBRA_BENCH_CONCURRENCY", "1,2,4,8,16,32");
    let seconds = env_list("ZEBRA_BENCH_SECONDS", "5")[0];
    let port = env_list("ZEBRA_BENCH_PORT", "28232")[0] as u16;
    let select = env::var("ZEBRA_BENCH_SELECT").unwrap_or_else(|_| "uniform".to_string());
    let windows = env::var("ZEBRA_BENCH_WINDOWS").ok();

    let config = zebra_state::Config {
        cache_dir: cache_dir.clone().into(),
        ephemeral: false,
        ..zebra_state::Config::default()
    };

    println!("opening {cache_dir} read-only ({network})");
    let (read_state, db, _non_finalized_sender) = zebra_state::init_read_only(config, &network)
        .expect("the cached state directory should open read-only");

    let (tip_height, tip_hash) = db.tip().expect("the cached state should have a tip");
    println!("tip {tip_height:?} {tip_hash}");

    // Cached tip (`getblock` confirmations); sender outlives the benchmark (drop closes channel)
    let (chain_tip, chain_tip_sender) = MockChainTip::new();
    chain_tip_sender.send_best_tip_height(tip_height);
    chain_tip_sender.send_best_tip_hash(tip_hash);

    // Deep enough to measure the read state, not queueing in front of it
    let read_state = Buffer::new(read_state, 8192);

    let (rpc, tx_queue) = RpcImpl::new(
        network.clone(),
        Default::default(),
        false,
        "0.0.1",
        "getblock benchmark",
        MockService::build().for_unit_tests(),
        MockService::build().for_unit_tests(),
        read_state,
        MockService::build().for_unit_tests(),
        MockSyncStatus::default(),
        chain_tip,
        MockAddressBookPeers::default(),
        tokio::sync::watch::channel(None).1,
        None,
    );

    // Polls mempool & state on a timer (unexpected requests to the mocks); unused by `getblock`
    tx_queue.abort();

    let listen_addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let rpc_config = zebra_rpc::config::rpc::Config {
        listen_addr: Some(listen_addr),
        enable_cookie_auth: false,
        // Verbosity 2 spam-era blocks render far past the shipped default
        max_response_body_size: 256 * 1024 * 1024,
        ..Default::default()
    };

    let _server = RpcServer::start(rpc, rpc_config)
        .await
        .expect("the benchmark's RPC server should bind");

    let client = RpcRequestClient::new(listen_addr);

    let windows = match &windows {
        Some(spec) => Window::parse(spec, tip_height),
        None => vec![Window {
            name: "tip".to_string(),
            lowest: tip_height.0.saturating_sub(10_000),
            highest: tip_height.0,
        }],
    };

    let pools: Vec<(&Window, Arc<Vec<u32>>)> = windows
        .iter()
        .map(|window| {
            let pool = Arc::new(height_pool(&db, window, select.as_str()));
            assert!(
                !pool.is_empty(),
                "window {} ({}..{}) has no blocks at or below the tip",
                window.name,
                window.lowest,
                window.highest
            );
            Composition::of(&db, &pool).report(&format!(
                "{} ({}..{}, {select})",
                window.name, window.lowest, window.highest
            ));
            (window, pool)
        })
        .collect();

    // Warm-up: touch every sampled block once (page faults, RocksDB block-cache misses)
    // - First response per verbosity checked (else JSON-RPC errors time as served blocks)
    for &verbosity in &verbosities {
        let mut checked = false;
        for (_, pool) in &pools {
            for height in pool.iter() {
                if !checked {
                    let body = client
                        .call("getblock", format!(r#"["{height}", {verbosity}]"#))
                        .await
                        .expect("the benchmark's server should answer a warm-up call")
                        .text()
                        .await
                        .expect("a warm-up response should be a body");

                    assert!(
                        body.contains(r#""result""#) && !body.contains(r#""error":{"#),
                        "getblock should return a result, got: {}",
                        &body[..body.len().min(200)]
                    );
                    checked = true;
                    continue;
                }

                let _ = request(&client, *height, verbosity).await;
            }
        }
    }

    println!(
        "\n{seconds}s per measurement\n\n\
         {:>14}  {:>4}  {:>5}  {:>10}  {:>9}  {:>9}  {:>9}  {:>9}  {:>7}",
        "window", "verb", "conc", "req/s", "MiB/s", "p50 ms", "p95 ms", "p99 ms", "errors"
    );

    for (window, pool) in &pools {
        for &verbosity in &verbosities {
            for concurrency in concurrencies.iter().map(|c| *c as usize) {
                let measurement =
                    measure(&client, verbosity, concurrency, pool.clone(), seconds).await;

                println!(
                    "{:>14}  {:>4}  {:>5}  {:>10.1}  {:>9.1}  {:>9.2}  {:>9.2}  {:>9.2}  {:>7}",
                    window.name,
                    verbosity,
                    measurement.concurrency,
                    measurement.per_second(),
                    measurement.mib_per_second(),
                    measurement.quantile(0.50),
                    measurement.quantile(0.95),
                    measurement.quantile(0.99),
                    measurement.errors,
                );
            }
        }
    }
}

/// One `getblock` over HTTP, body drained to the last byte, returns its length
///
/// - Full body (consumer latency != time to first header)
/// - Bytes, not text (UTF-8 validation of MiBs of hex = client work in the measurement)
async fn request(client: &RpcRequestClient, height: u32, verbosity: u8) -> Option<usize> {
    let response = client
        .call("getblock", format!(r#"["{height}", {verbosity}]"#))
        .await
        .ok()?;

    response.bytes().await.ok().map(|body| body.len())
}

/// `concurrency` requests in flight for `seconds`
async fn measure(
    client: &RpcRequestClient,
    verbosity: u8,
    concurrency: usize,
    pool: Arc<Vec<u32>>,
    seconds: u64,
) -> Measurement {
    let window = Duration::from_secs(seconds);
    let started = Instant::now();

    let workers: Vec<_> = (0..concurrency)
        .map(|worker| {
            let client = client.clone();
            let mut heights = Heights::new(pool.clone(), worker);

            tokio::spawn(async move {
                let mut latencies = Vec::new();
                let mut errors = 0u64;
                let mut bytes = 0u64;

                while started.elapsed() < window {
                    let height = heights.next();
                    let call = Instant::now();

                    match request(&client, height, verbosity).await {
                        Some(len) => {
                            bytes += len as u64;
                            latencies.push(call.elapsed().as_micros() as u64);
                        }
                        None => errors += 1,
                    }
                }

                (latencies, errors, bytes)
            })
        })
        .collect();

    let mut latencies = Vec::new();
    let mut errors = 0;
    let mut bytes = 0;

    for worker in workers {
        let (worker_latencies, worker_errors, worker_bytes) =
            worker.await.expect("a benchmark worker should not panic");

        latencies.extend(worker_latencies);
        errors += worker_errors;
        bytes += worker_bytes;
    }

    Measurement {
        concurrency,
        requests: latencies.len() as u64,
        errors,
        bytes,
        elapsed: started.elapsed(),
        latencies,
    }
}
