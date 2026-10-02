//! `laya` command-line interface. Ported from `laya/cli.py`.
//!
//!   laya "I was charged twice, please refund"        # routing decision only (no download)
//!   laya "Refactor this service" --predict           # full answers (downloads the checkpoint)
//!   laya                                             # interactive mode
//!   laya "Mein Konto wurde zweimal belastet" --lang de
//!   laya "My payment failed twice" --lang-guess en   # a soft hint that still lets routing decide
//!
//! Routing (the default) never downloads a checkpoint, so it works offline and returns in
//! milliseconds. `--predict` loads the routed checkpoint on first use (needs Hub access).

use std::io::{self, Write};

use clap::Parser;
use laya::router::{RouteHints, Router, RouterOptions};
use laya::{
    email_questions, guard_questions, moderation_questions, router_questions, triage_questions,
    Questions,
};
use serde_json::{json, Value};

/// Ready-made question presets a prediction can run instead of `router_questions()`.
const PRESETS: [&str; 5] = ["email", "guard", "moderation", "router", "triage"];

/// The state field each preset's instructions name, so the CLI puts the text where that question
/// set reads it (upstream `fix/cli-preset-state-key`). `--predict` without a preset answers
/// `router_questions()`, whose field is `request`, which is also the default. Routing is unaffected:
/// `route` reads the state only for language detection, which is key-invariant.
fn preset_state_key(preset: Option<&str>) -> &'static str {
    match preset {
        Some("email") => "body",
        Some("guard") => "prompt",
        Some("moderation") => "post",
        Some("triage") => "message",
        // "router", and the plain --predict path with no preset.
        _ => "request",
    }
}

/// Build a preset's questions by name. `name` is validated by clap against [`PRESETS`].
fn preset_questions(name: &str) -> Questions {
    match name {
        "email" => email_questions(None),
        "guard" => guard_questions(),
        "moderation" => moderation_questions(),
        "router" => router_questions(),
        "triage" => triage_questions(),
        _ => router_questions(),
    }
}

#[derive(Parser)]
#[command(
    name = "laya",
    about = "Test Laya locally: route or answer a request from the command line."
)]
struct Cli {
    /// The request text (omit for interactive mode).
    text: Vec<String>,
    /// Run the full prediction, not just the routing decision (downloads the checkpoint on first use).
    #[arg(long)]
    predict: bool,
    /// Force a checkpoint instead of auto-routing: a checkpoint name or any of the router's
    /// aliases (en, ml, td, ...), in any casing, or 'auto' to route it (the default).
    #[arg(long)]
    model: Option<String>,
    /// Force a language, e.g. en or de, instead of detecting it.
    #[arg(long)]
    lang: Option<String>,
    /// A soft language hint that participates in routing instead of skipping detection: checked
    /// after --lang and before the built-in detector, so a probable-but-uncertain code can nudge
    /// the checkpoint without forcing it.
    #[arg(long, value_name = "CODE")]
    lang_guess: Option<String>,
    /// Force a typed-decisions workflow instead of detecting it.
    #[arg(long)]
    task: Option<String>,
    /// Answer a ready-made question preset (email, guard, moderation, router, triage)
    /// instead of the router questions; implies --predict.
    #[arg(long, value_name = "NAME", value_parser = PRESETS)]
    preset: Option<String>,
    /// Compute device, e.g. cpu or metal.
    #[arg(long)]
    device: Option<String>,
    /// Print the raw result as JSON.
    #[arg(long)]
    json: bool,
}

fn make_router(cli: &Cli) -> Router {
    Router::new(RouterOptions {
        device: cli.device.clone(),
        ..Default::default()
    })
    .expect("router options")
}

fn hints(cli: &Cli) -> RouteHints<'_> {
    RouteHints {
        // `--model auto` means "do not pin one", exactly as omitting the flag does, so it maps to
        // None; any other value (a name or an alias like `en`) reaches `route`, which resolves it
        // through `normalise_name` (upstream cli `model_name`).
        model: cli
            .model
            .as_deref()
            .filter(|m| !m.trim().eq_ignore_ascii_case("auto")),
        task: cli.task.as_deref(),
        lang: cli.lang.as_deref(),
        // A soft hint checked after `--lang` and before the built-in detector (upstream cli
        // `--lang-guess`): it nudges routing without forcing a checkpoint.
        lang_guess: cli.lang_guess.as_deref(),
    }
}

fn show_decision(d: &Value) {
    println!("Model     : {}", d["model"].as_str().unwrap_or(""));
    println!("Reason    : {}", d["reason"].as_str().unwrap_or(""));
    if let Some(det) = d.get("detection") {
        if !det.is_null() {
            println!(
                "Detected  : {}",
                serde_json::to_string(det).unwrap_or_default()
            );
        }
    }
}

fn show_answers(result: &Value) {
    if let Some(routing) = result.get("routing") {
        if !routing.is_null() {
            show_decision(routing);
            println!();
        }
    }
    if let Some(answers) = result.get("answers").and_then(|a| a.as_object()) {
        for (qid, ans) in answers {
            let detail = if let Some(c) = ans.get("choice").and_then(|v| v.as_str()) {
                let p = ans
                    .get("probabilities")
                    .and_then(|p| p.get(c))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                format!("{} (p={:.3})", c, p)
            } else if let Some(s) = ans.get("score").and_then(|v| v.as_f64()) {
                format!("{:.2}", s)
            } else if let Some(n) = ans.get("noul").and_then(|v| v.as_f64()) {
                format!("{:.3}", n)
            } else {
                serde_json::to_string(ans).unwrap_or_default()
            };
            println!("{:<12}: {}", qid, detail);
        }
    }
}

/// Route or predict one request; returns 0 on success, 2 on a handled error.
fn run(text: &str, cli: &Cli, router: &Router) -> i32 {
    let h = hints(cli);
    if cli.predict || cli.preset.is_some() {
        let questions = match &cli.preset {
            Some(name) => preset_questions(name),
            None => router_questions(),
        };
        // Send the text under the field the answered question set names, not a fixed `text` key
        // none of them reads (upstream fix/cli-preset-state-key).
        let state = json!({ preset_state_key(cli.preset.as_deref()): text });
        match router.predict(&state, &questions, &h) {
            Ok(result) => {
                let j = result.to_json();
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&j).unwrap_or_default());
                } else {
                    show_answers(&j);
                }
                0
            }
            Err(e) => {
                eprintln!("laya: {e}");
                eprintln!(
                    "Check that the checkpoints can be downloaded from the Hugging Face hub \
                     (network access is needed on first use)."
                );
                2
            }
        }
    } else {
        let state = json!({ "text": text });
        match router.route(&state, None, &h) {
            Ok(d) => {
                let j = d.to_json();
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&j).unwrap_or_default());
                } else {
                    show_decision(&j);
                }
                0
            }
            Err(e) => {
                eprintln!("laya: {e}");
                2
            }
        }
    }
}

fn interactive(cli: &Cli, router: &Router) -> i32 {
    println!("Laya interactive mode. Type a request and press Enter; Ctrl-D or 'quit' to exit.");
    loop {
        print!("laya> ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        match io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => {
                println!();
                break;
            }
            Ok(_) => {}
        }
        let t = line.trim();
        if t.is_empty() || t.eq_ignore_ascii_case("quit") || t.eq_ignore_ascii_case("exit") {
            break;
        }
        run(t, cli, router);
    }
    0
}

fn main() {
    let cli = Cli::parse();
    let text = cli.text.join(" ").trim().to_string();
    let router = make_router(&cli);
    let code = if text.is_empty() {
        interactive(&cli, &router)
    } else {
        run(&text, &cli, &router)
    };
    std::process::exit(code);
}
