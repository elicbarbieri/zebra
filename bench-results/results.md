# Note commitment tree updates on real mainnet blocks: #11846 vs batched MerkleCRH

`cargo bench -p zebra-chain --bench orchard_tree` (data from `zebra-chain/benches/fetch_replay.py`).
The tree is seeded from `z_gettreestate` before each window and every block is replayed through
`update_orchard_note_commitment_tree` as sync does; the replay must reproduce the real final root
(it does, on all three variants). `block/*` times one real block from its real pre-state.

Variants (branches on elicbarbieri/zebra): **A** parent `deps/sinsemilla-fork` 5fa5b6fa6 (`bench/base`) ·
**B** #11846 head 9402d8fd3 (`bench/arm-b`) · **C** parent + zcash/sinsemilla#9+#17 +
incrementalmerkletree#219 (`bench/arm-c`). i9-13900H pinned to P-cores, performance EPP, 2026-11-05.
Medians, ± half the 95 % CI; replay = 10 samples, blocks = 100.

| window | heights | blocks | with commitments | commitments | per non-empty block p50 / p90 / max |
|---|---|---:|---:|---:|---|
| orchard-sandblast | 1,831,000–1,833,000 | 2,001 | 72 % | 360,314 | 242 / 463 / 631 |
| ironwood-tip | 3,533,000–3,543,000 | 10,001 | 60 % | 94,862 | 6 / 17 / 439 |

### RAYON_NUM_THREADS=4 (median ms ± CI95/2)

| benchmark | A | B | C | A/B | A/C | B/C |
|---|---:|---:|---:|---:|---:|---:|
| orchard-sandblast/replay | 13,232 ± 169 | 4,417 ± 34 | 1,769 ± 12 | 3.00× | 7.48× | 2.50× |
| orchard-sandblast/block/p50-242 | 8.76 ± 0.03 | 2.98 ± 0.01 | 1.20 ± 0.02 | 2.94× | 7.29× | 2.48× |
| orchard-sandblast/block/p90-463 | 16.06 ± 0.07 | 5.41 ± 0.02 | 1.74 ± 0.04 | 2.97× | 9.21× | 3.10× |
| orchard-sandblast/block/max-631 | 21.28 ± 0.05 | 7.15 ± 0.04 | 1.99 ± 0.04 | 2.97× | 10.67× | 3.59× |
| ironwood-tip/replay | 9,562 ± 81 | 3,076 ± 10 | 3,048 ± 195 | 3.11× | 3.14× | 1.01× |
| ironwood-tip/block/p50-6 | 1.36 ± 0.00 | 0.42 ± 0.00 | 0.46 ± 0.00 | 3.22× | 2.96× | 0.92× |
| ironwood-tip/block/p90-17 | 1.60 ± 0.01 | 0.50 ± 0.00 | 0.56 ± 0.00 | 3.21× | 2.84× | 0.88× |
| ironwood-tip/block/max-439 | 15.57 ± 0.05 | 5.01 ± 0.02 | 1.59 ± 0.03 | 3.11× | 9.82× | 3.16× |

### RAYON_NUM_THREADS=1

| benchmark | A | B | C | A/B | A/C | B/C |
|---|---:|---:|---:|---:|---:|---:|
| orchard-sandblast/replay | 13,172 ± 34 | 4,390 ± 8 | 2,953 ± 12 | 3.00× | 4.46× | 1.49× |
| orchard-sandblast/block/p50-242 | 8.71 ± 0.02 | 2.89 ± 0.01 | 1.97 ± 0.00 | 3.01× | 4.42× | 1.46× |
| orchard-sandblast/block/p90-463 | 15.97 ± 0.07 | 5.34 ± 0.02 | 3.26 ± 0.01 | 2.99× | 4.89× | 1.64× |
| orchard-sandblast/block/max-631 | 21.25 ± 0.05 | 7.08 ± 0.03 | 4.19 ± 0.01 | 3.00× | 5.07× | 1.69× |
| ironwood-tip/replay | 9,292 ± 27 | 3,053 ± 7 | 3,136 ± 20 | 3.04× | 2.96× | 0.97× |
| ironwood-tip/block/p50-6 | 1.31 ± 0.00 | 0.42 ± 0.00 | 0.45 ± 0.00 | 3.12× | 2.92× | 0.93× |
| ironwood-tip/block/p90-17 | 1.54 ± 0.00 | 0.49 ± 0.00 | 0.54 ± 0.00 | 3.14× | 2.86× | 0.91× |
| ironwood-tip/block/max-439 | 15.00 ± 0.03 | 5.00 ± 0.02 | 3.05 ± 0.01 | 3.00× | 4.92× | 1.64× |

### Reading

- **Syncing the sandblast era:** per block the tree costs A 6.6 ms, B 2.2 ms, C 1.5 ms single-threaded and
  0.9 ms with four rayon threads. Batched is 1.5× (1 thread) to 2.5× (4 threads) over #11846 across the
  whole window, 1.6–3.6× on the p90/max blocks.
- **Validating at today's tip:** median Ironwood block has 6 commitments, so the 32-hash root chain
  dominates and the two are equal (B 7 % ahead on the median block, C 3× ahead on the 439-action bursts).
- A and B do not change with threads; only C's level-wise `combine_pairs` does.
- Variant C's bench must be built with `SINSEMILLA_TABLE_LIMIT=3473408` (compile-time table budget for
  the 52-word MerkleCRH message; the 1 MiB Linux default silently falls back to untabled hashing).
