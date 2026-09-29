//! End-to-end numerical parity gate: candle backend vs. the upstream PyTorch reference.
//!
//! Reads `tests/golden_e2e/<checkpoint>.json` (produced by `scripts/dump_e2e.py` in a torch
//! environment), loads the same checkpoint via candle, runs the same (state, questions), and
//! asserts the answers match within tolerance.
//!
//! Opt-in (needs network + weights + the reference file):
//!   cargo test -p laya --release --test e2e -- --ignored --nocapture
//!
//! `LAYA_TEST_DEVICE` selects the backend, so the same parity gate can gate a GPU build. Building
//! with the backend's feature off falls back to CPU, so check the printed line before trusting a
//! green run:
//!   LAYA_TEST_DEVICE=cuda cargo test -p laya-decision --release --features cuda \
//!       --test e2e -- --ignored --nocapture
#![cfg(feature = "model")]

use laya::agent::{Agent, LoadOptions};
use laya::Questions;
use serde_json::Value;
use std::path::PathBuf;

/// Absolute tolerance for probabilities / scores / confidences (both sides round to 4 dp).
const TOL: f64 = 2e-3;

fn checkpoint_repo(stem: &str) -> (&'static str, Option<String>) {
    match stem {
        "english" => ("convaiinnovations/laya", None),
        "multilingual" => ("convaiinnovations/laya", Some("multilingual".to_string())),
        "typed-decisions" => (
            "convaiinnovations/laya",
            Some("typed-decisions".to_string()),
        ),
        _ => panic!("unknown checkpoint stem {stem}"),
    }
}

fn as_questions(v: &Value) -> Questions {
    v.as_object()
        .expect("questions object")
        .iter()
        .map(|(k, val)| (k.clone(), val.clone()))
        .collect()
}

fn check_checkpoint(stem: &str, max_diff: &mut f64, mismatches: &mut Vec<String>) -> bool {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "tests",
        "golden_e2e",
        &format!("{stem}.json"),
    ]
    .iter()
    .collect();
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => {
            eprintln!(
                "skip {stem}: no reference at {} (run scripts/dump_e2e.py)",
                path.display()
            );
            return false;
        }
    };
    let rows: Vec<Value> = serde_json::from_slice(&bytes).expect("valid e2e json");
    let (repo, subfolder) = checkpoint_repo(stem);
    let device = std::env::var("LAYA_TEST_DEVICE")
        .ok()
        .filter(|d| !d.is_empty());
    let agent = Agent::load(
        repo,
        LoadOptions {
            subfolder,
            device: device.clone(),
            ..Default::default()
        },
    )
    .expect("load checkpoint");

    for row in &rows {
        let state = row["state"].clone();
        let questions = as_questions(&row["questions"]);
        let want = &row["result"]["answers"];
        let got = agent
            .system_one(&state, &questions)
            .expect("system_one")
            .to_json();
        let got = &got["answers"];

        for (qid, wa) in want.as_object().unwrap() {
            let ga = &got[qid];
            let qs = row["question_set"].as_str().unwrap_or("?");
            let ctx = format!("[{stem}/{qs}] {qid}");
            // choice label must match exactly
            if let Some(wc) = wa.get("choice").and_then(|v| v.as_str()) {
                let gc = ga.get("choice").and_then(|v| v.as_str()).unwrap_or("");
                if wc != gc {
                    mismatches.push(format!("{ctx}: choice {gc:?} != {wc:?}"));
                }
            }
            // compare every numeric leaf (score, noul, confidence, probabilities.*, act_probability)
            compare_numbers(wa, ga, &ctx, max_diff, mismatches);
        }
    }
    true
}

fn compare_numbers(
    want: &Value,
    got: &Value,
    ctx: &str,
    max_diff: &mut f64,
    mismatches: &mut Vec<String>,
) {
    match want {
        Value::Object(m) => {
            for (k, wv) in m {
                if let Some(gv) = got.get(k) {
                    compare_numbers(wv, gv, &format!("{ctx}.{k}"), max_diff, mismatches);
                }
            }
        }
        Value::Number(wn) => {
            if let (Some(w), Some(g)) = (wn.as_f64(), got.as_f64()) {
                let d = (w - g).abs();
                if d > *max_diff {
                    *max_diff = d;
                }
                if d > TOL {
                    mismatches.push(format!("{ctx}: {g} != {w} (|Δ|={d:.5})"));
                }
            }
        }
        _ => {}
    }
}

#[test]
#[ignore = "needs weights + a torch reference; run with --ignored"]
fn e2e_parity() {
    let requested = std::env::var("LAYA_TEST_DEVICE").unwrap_or_else(|_| "cpu".into());
    println!("e2e parity: requested device = {requested:?}");
    // Without the feature the backend silently degrades to CPU, so a green run would be CPU
    // numbers wearing a CUDA label. Fail instead of reporting a parity result for another device.
    let name = requested.trim().to_lowercase();
    let missing = |f: &str| {
        format!(
            "LAYA_TEST_DEVICE={requested:?} but the `{f}` feature is not enabled for \
             laya-decision; rebuild with `--features {f}` or the run silently tests CPU"
        )
    };
    // `cfg!` folds at compile time, which is the intent: this asserts the build carries the
    // feature, so a missing feature fails the run instead of quietly producing CPU numbers.
    #[allow(clippy::assertions_on_constants)]
    if name == "cuda" {
        assert!(cfg!(feature = "cuda"), "{}", missing("cuda"));
    } else if name == "metal" {
        assert!(cfg!(feature = "metal"), "{}", missing("metal"));
    }
    let mut max_diff = 0.0f64;
    let mut mismatches = Vec::new();
    let mut ran = false;
    for stem in ["english", "multilingual", "typed-decisions"] {
        ran |= check_checkpoint(stem, &mut max_diff, &mut mismatches);
    }
    if !ran {
        eprintln!("e2e_parity: no reference files found; nothing checked.");
        return;
    }
    println!("e2e parity: max |Δ| = {max_diff:.6} (tolerance {TOL}) on {requested}");
    assert!(
        mismatches.is_empty(),
        "e2e parity mismatches ({}):\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}
