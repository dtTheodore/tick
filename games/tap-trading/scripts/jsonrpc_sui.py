#!/usr/bin/env python3
"""Submit Sui transactions over JSON-RPC, signing offline with `sui keytool sign`.

This bypasses the `sui` CLI's gRPC fullnode entirely (build + execute over
JSON-RPC, sign offline) — the durable form of the SettlerClient trait swap. Used
by the USDC e2e because testnet's gRPC pool currently won't serve the
externally-faucet'd USDC coin to the CLI, while JSON-RPC nodes have it.
"""
import json
import subprocess
import sys
import urllib.request

RPC = "https://fullnode.testnet.sui.io:443"


def rpc(method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    req = urllib.request.Request(RPC, data=body, headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=30) as r:
        d = json.loads(r.read())
    if "error" in d:
        raise RuntimeError(f"{method}: {d['error']}")
    return d["result"]


def sign(signer, tx_bytes_b64):
    """Offline-sign BCS tx bytes with the keystore key (no RPC, no gRPC)."""
    out = subprocess.run(
        ["sui", "keytool", "sign", "--address", signer, "--data", tx_bytes_b64, "--json"],
        capture_output=True, text=True, check=True,
    )
    return json.loads(out.stdout)["suiSignature"]


def execute(signer, tx_bytes):
    sig = sign(signer, tx_bytes)
    return rpc("sui_executeTransactionBlock", [
        tx_bytes, [sig],
        {"showEffects": True, "showObjectChanges": True, "showBalanceChanges": True},
        "WaitForLocalExecution",
    ])


def assert_ok(res, label):
    st = res["effects"]["status"]["status"]
    if st != "success":
        raise RuntimeError(f"{label} failed: {res['effects']['status']}")
    return res


def move_call(signer, pkg, module, func, type_args, args, gas_budget="200000000"):
    built = rpc("unsafe_moveCall", [signer, pkg, module, func, type_args, args, None, gas_budget, None])
    return assert_ok(execute(signer, built["txBytes"]), f"{module}::{func}")


def split_coin(signer, coin_id, amounts, gas_budget="200000000"):
    built = rpc("unsafe_splitCoin", [signer, coin_id, [str(a) for a in amounts], None, gas_budget])
    return assert_ok(execute(signer, built["txBytes"]), "split_coin")


def created(res, type_substr):
    for c in res.get("objectChanges", []):
        if c.get("type") == "created" and type_substr in c.get("objectType", ""):
            return c["objectId"]
    raise RuntimeError(f"no created object matching {type_substr}")


def created_all(res, type_substr):
    return [c["objectId"] for c in res.get("objectChanges", [])
            if c.get("type") == "created" and type_substr in c.get("objectType", "")]


if __name__ == "__main__":
    # Self-test: split the faucet USDC coin via JSON-RPC + offline sign.
    signer, coin = sys.argv[1], sys.argv[2]
    res = split_coin(signer, coin, [5000000])
    print("split status:", res["effects"]["status"]["status"])
    print("created USDC coins:", created_all(res, "Coin<"))
