# laya-decision-cli

Command-line interface for [**Laya-Decision**](https://github.com/GTC6244/Laya-Decision) — a
pure-Rust port of the Laya non-autoregressive System-1 decision engine. Installs the `laya`
binary.

```bash
cargo install laya-decision-cli
```

```bash
laya "I was charged twice, please refund"      # routing decision only (offline, no download)
laya "Refactor this service" --predict         # full answers (downloads the checkpoint)
laya "Mein Konto wurde zweimal belastet" --lang de
laya                                           # interactive mode
```

Options: `--predict`, `--model`, `--task`, `--lang`, `--device`, `--json`. Routing is offline and
returns in milliseconds; `--predict` loads the routed checkpoint from the Hugging Face Hub on
first use.

See the [repository](https://github.com/GTC6244/Laya-Decision) for the library
([`laya-decision`](https://crates.io/crates/laya-decision)) and the HTTP server
([`laya-decision-serve`](https://crates.io/crates/laya-decision-serve)).

Apache-2.0. A port of upstream [Laya](https://github.com/NandhaKishorM/laya) (Apache-2.0).
