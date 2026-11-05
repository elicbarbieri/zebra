#!/usr/bin/env python3
"""Fetch a mainnet block window for the `orchard_tree` replay bench.

  fetch_replay.py --rpc http://host:8232 --grpc host:443 --proto-dir DIR \
      --out bench-results/replay/<name> --pool orchard|ironwood --start H --end H

Writes `window.json` (pool, heights, z_gettreestate frontier before `start`, final root at `end`)
and `blocks.bin`: per block `height:u32le n:u32le cmx[n]:32 bytes`, straight from Zaino's
compact blocks. Needs `grpcurl` on PATH (e.g. `nix shell nixpkgs#grpcurl`).
"""
import argparse
import base64
import json
import struct
import subprocess
import urllib.request
from pathlib import Path


def rpc(url, method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(url, body, {"content-type": "application/json"})
    reply = json.load(urllib.request.urlopen(req, timeout=120))
    if reply.get("error"):
        raise RuntimeError(f"{method}: {reply['error']}")
    return reply["result"]


def compact_blocks(grpc, proto_dir, pool, start, end):
    """Yields (height, [cmx bytes]) for start..=end; `ironwoodActions` or `actions` per pool"""
    rng = {"start": {"height": start}, "end": {"height": end}, "poolTypes": [pool.upper()]}
    cmd = ["grpcurl", "-import-path", proto_dir, "-proto", "service.proto", "-d", json.dumps(rng),
           grpc, "cash.z.wallet.sdk.rpc.CompactTxStreamer/GetBlockRange"]
    field = "ironwoodActions" if pool == "ironwood" else "actions"
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, text=True)
    dec, buf = json.JSONDecoder(), ""
    for line in proc.stdout:
        buf += line
        while buf.strip():
            try:
                obj, end_idx = dec.raw_decode(buf.lstrip())
            except json.JSONDecodeError:
                break
            buf = buf.lstrip()[end_idx:]
            cmxs = [base64.b64decode(a["cmx"]) for tx in obj.get("vtx", []) for a in tx.get(field, [])]
            yield int(obj["height"]), cmxs
    if proc.wait() != 0:
        raise RuntimeError("grpcurl failed")


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--rpc", required=True)
    p.add_argument("--grpc", required=True)
    p.add_argument("--proto-dir", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--pool", choices=["orchard", "ironwood"], required=True)
    p.add_argument("--start", type=int, required=True)
    p.add_argument("--end", type=int, required=True)
    a = p.parse_args()

    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    before = rpc(a.rpc, "z_gettreestate", [str(a.start - 1)])[a.pool]["commitments"]
    after = rpc(a.rpc, "z_gettreestate", [str(a.end)])[a.pool]["commitments"]

    blocks = commitments = 0
    with open(out / "blocks.bin", "wb") as f:
        for height, cmxs in compact_blocks(a.grpc, a.proto_dir, a.pool, a.start, a.end):
            f.write(struct.pack("<II", height, len(cmxs)) + b"".join(cmxs))
            blocks += 1
            commitments += len(cmxs)
            if blocks % 500 == 0:
                print(f"{height}: {blocks} blocks, {commitments} commitments", flush=True)
    assert blocks == a.end - a.start + 1, f"got {blocks} blocks"

    (out / "window.json").write_text(json.dumps({
        "pool": a.pool, "start": a.start, "end": a.end, "blocks": blocks, "commitments": commitments,
        "start_state": before["finalState"], "end_root": after["finalRoot"],
    }, indent=2))
    print(f"{a.pool} {a.start}..={a.end}: {blocks} blocks, {commitments} commitments → {out}")


if __name__ == "__main__":
    main()
