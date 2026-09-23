# laya-decision-serve

A Jev-compatible HTTP server for [**Laya-Decision**](https://github.com/GTC6244/Laya-Decision) —
a pure-Rust port of the Laya non-autoregressive System-1 decision engine. Installs the
`laya-serve` binary.

```bash
cargo install laya-decision-serve
laya-serve   # binds 0.0.0.0:8000 by default
```

```bash
curl -s localhost:8000/v1/systemone -H 'content-type: application/json' -d '{
  "state": {"message": "Mein Konto wurde zweimal belastet"},
  "questions": {"is_urgent": {"type": "noul", "instructions": "Is this urgent?"}}
}'
```

Endpoints: `POST /v1/systemone` (TypeSafe Jev wire protocol — `{model, answers, usage, routing}`)
and `GET /health`. The `Router` auto-selects the English or multilingual checkpoint per request.

Configuration is via environment variables: `LAYA_HOST`, `LAYA_PORT`, `LAYA_DEVICE`
(`cpu`/`metal`), `LAYA_PRELOAD`, `LAYA_MODELS`, `LAYA_AUTO_TASK`, `LAYA_API_KEY`, `LAYA_LOG_LEVEL`.

See the [repository](https://github.com/GTC6244/Laya-Decision) for the library
([`laya-decision`](https://crates.io/crates/laya-decision)) and the CLI
([`laya-decision-cli`](https://crates.io/crates/laya-decision-cli)).

Apache-2.0. A port of upstream [Laya](https://github.com/NandhaKishorM/laya) (Apache-2.0).
