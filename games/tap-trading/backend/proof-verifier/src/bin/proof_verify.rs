//! `proof-verify <proof.json | ->` — replay a Walrus proof blob and report the
//! verdict. Reads a file path, or `-` for stdin (so you can pipe
//! `walrus read <blob_id> | proof-verify -`). Exits non-zero on any non-Valid
//! verdict so it composes in scripts and CI.

use std::io::Read;
use std::process::ExitCode;

use tap_trading_proof_types::ProofBlob;
use tap_trading_proof_verifier::{verify, VerifyResult};

fn main() -> ExitCode {
    let arg = std::env::args().nth(1);
    let Some(arg) = arg else {
        eprintln!("usage: proof-verify <proof.json | ->");
        return ExitCode::from(2);
    };

    let json = if arg == "-" {
        let mut s = String::new();
        if std::io::stdin().read_to_string(&mut s).is_err() {
            eprintln!("error: failed to read stdin");
            return ExitCode::from(2);
        }
        s
    } else {
        match std::fs::read_to_string(&arg) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: cannot read {arg}: {e}");
                return ExitCode::from(2);
            }
        }
    };

    let blob: ProofBlob = match serde_json::from_str(&json) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error: invalid proof JSON: {e}");
            return ExitCode::from(2);
        }
    };

    match verify(&blob) {
        VerifyResult::Valid => {
            println!("Valid — position {} ({:?})", blob.position_id, blob.settlement.outcome);
            ExitCode::SUCCESS
        }
        other => {
            println!("INVALID: {other:?}");
            ExitCode::FAILURE
        }
    }
}
