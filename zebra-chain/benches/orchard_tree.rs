//! Orchard / Ironwood note commitment tree updates replayed over real mainnet blocks.
//!
//! - data: `bench-results/replay/<window>/{window.json,blocks.bin}` from `benches/fetch_replay.py`
//!   (directory override: `ORCHARD_REPLAY`)
//! - tree seeded from `z_gettreestate` before the window; the replay must reproduce the real final root
//! - `replay` = every block in order, as sync does; `block/p50|p90|max-<n>` = one real block from its
//!   real pre-state (empty blocks skipped, as the block path skips them)

#![allow(missing_docs)]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use halo2::pasta::{group::ff::PrimeField, pallas};
use serde_json::json;
use zcash_primitives::merkle_tree::read_commitment_tree;

use zebra_chain::{
    orchard::tree::{legacy::LegacyFrontier, Node, NoteCommitmentTree},
    parallel::tree::NoteCommitmentTrees,
};

type Block = Vec<pallas::Base>;

struct Window {
    name: String,
    end_root: String,
    start: NoteCommitmentTree,
    blocks: Vec<Block>,
}

fn replay_dir() -> PathBuf {
    std::env::var_os("ORCHARD_REPLAY")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench-results/replay"))
}

fn load(dir: &Path) -> Window {
    let meta: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("window.json")).expect("window.json")).unwrap();
    let state = hex::decode(meta["start_state"].as_str().unwrap()).unwrap();
    let frontier = read_commitment_tree::<Node, _, 32>(&state[..])
        .expect("z_gettreestate finalState")
        .to_frontier();
    let start: NoteCommitmentTree = serde_json::from_value(json!({
        "inner": LegacyFrontier::<Node, 32>::from(frontier),
        "cached_root": null,
    }))
    .expect("tree from frontier");

    let bin = fs::read(dir.join("blocks.bin")).expect("blocks.bin");
    let (mut blocks, mut at) = (Vec::new(), 0);
    while at < bin.len() {
        let n = u32::from_le_bytes(bin[at + 4..at + 8].try_into().unwrap()) as usize;
        at += 8;
        let block = bin[at..at + 32 * n]
            .as_chunks::<32>()
            .0
            .iter()
            .map(|cmx| pallas::Base::from_repr(*cmx).expect("canonical cmx"))
            .collect();
        at += 32 * n;
        blocks.push(block);
    }
    Window {
        name: dir.file_name().unwrap().to_string_lossy().into_owned(),
        end_root: meta["end_root"].as_str().unwrap().to_owned(),
        start,
        blocks,
    }
}

fn update(tree: Arc<NoteCommitmentTree>, block: Block) -> Arc<NoteCommitmentTree> {
    NoteCommitmentTrees::update_orchard_note_commitment_tree(tree, block)
        .expect("below capacity")
        .0
}

fn replay(window: &Window) {
    let dir = replay_dir().join(&window.name);
    let mut widths: Vec<usize> = window
        .blocks
        .iter()
        .map(Vec::len)
        .filter(|&n| n > 0)
        .collect();
    widths.sort_unstable();
    let picks = [
        ("p50", widths[widths.len() / 2]),
        ("p90", widths[widths.len() * 9 / 10]),
        ("max", widths[widths.len() - 1]),
    ];

    // one untimed replay: real final root, plus the pre-state of each picked block
    let mut tree = Arc::new(window.start.clone());
    let mut cases = Vec::new();
    for block in &window.blocks {
        if block.is_empty() {
            continue;
        }
        if let Some((label, _)) = picks.iter().find(|(_, w)| *w == block.len()) {
            if !cases.iter().any(|(l, _, _)| l == label) {
                cases.push((*label, Arc::clone(&tree), block.clone()));
            }
        }
        tree = update(tree, block.clone());
    }
    assert_eq!(
        hex::encode(tree.root().bytes_in_display_order()),
        window.end_root,
        "{}: replay did not reproduce the real final root",
        dir.display()
    );

    let mut c = Criterion::default().configure_from_args();
    let mut group = c.benchmark_group(&window.name);
    let commitments: usize = window.blocks.iter().map(Vec::len).sum();
    group
        .sample_size(10)
        .throughput(Throughput::Elements(commitments as u64));
    group.bench_function("replay", |b| {
        b.iter_batched(
            || (Arc::new(window.start.clone()), window.blocks.clone()),
            |(mut tree, blocks)| {
                for block in blocks.into_iter().filter(|b| !b.is_empty()) {
                    tree = update(tree, block);
                }
                tree
            },
            BatchSize::PerIteration,
        )
    });
    group.sample_size(100);
    for (label, pre, block) in &cases {
        group.throughput(Throughput::Elements(block.len() as u64));
        group.bench_function(format!("block/{label}-{}", block.len()), |b| {
            b.iter_batched(
                || (Arc::new(NoteCommitmentTree::clone(pre)), block.clone()),
                |(tree, block)| update(tree, block),
                BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}

fn windows(_: &mut Criterion) {
    let mut dirs: Vec<PathBuf> = fs::read_dir(replay_dir())
        .expect("replay data: run benches/fetch_replay.py")
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("window.json").exists())
        .collect();
    dirs.sort();
    for dir in dirs {
        replay(&load(&dir));
    }
}

criterion_group!(
    name = benches;
    config = Criterion::default().noise_threshold(0.03);
    targets = windows
);
criterion_main!(benches);
