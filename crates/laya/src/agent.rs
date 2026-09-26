//! High-level inference runtime. Ported from `laya/agent.py` (load, validate/normalize
//! questions, tokenize, forward, decode typed answers) — minus the TileLang fast path.

use std::path::{Path, PathBuf};

use candle_core::Device;
use indexmap::IndexMap;
use serde_json::{json, Map, Value};

use crate::common::{
    answer_confidence, build_sequence, clamp_temperature, collate_items, confidence_from_probs,
    render_options, round4, softmax, temp_bucket, InternalQ, Item, QType,
};
use crate::error::{LayaError, Result};
use crate::model::DecisionModel;
use crate::tokenizer::LayaTokenizer;
use crate::{Questions, State};

/// Options for [`Agent::load`].
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Compute device: `None`/`"cpu"` for CPU (the only backend built by default).
    pub device: Option<String>,
    /// Hugging Face token (falls back to `HF_TOKEN`).
    pub token: Option<String>,
    /// Subfolder within a bundled repo (e.g. `"multilingual"`).
    pub subfolder: Option<String>,
}

/// The result of a single-state evaluation (Jev-shaped).
#[derive(Debug, Clone)]
pub struct SystemOneResult {
    pub model: String,
    pub answers: IndexMap<String, Value>,
    pub input_tokens: usize,
    pub output_tokens: usize,
    /// Set by [`crate::Router::predict`]; the routing decision as JSON.
    pub routing: Option<Value>,
}

impl SystemOneResult {
    /// Serialise to the Jev wire shape: `{model, answers, usage, routing?}`.
    pub fn to_json(&self) -> Value {
        let answers: Map<String, Value> = self
            .answers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let mut out = json!({
            "model": self.model,
            "answers": Value::Object(answers),
            "usage": {"input_tokens": self.input_tokens, "output_tokens": self.output_tokens},
        });
        if let Some(r) = &self.routing {
            out.as_object_mut()
                .unwrap()
                .insert("routing".to_string(), r.clone());
        }
        out
    }
}

struct CheckpointFiles {
    config: PathBuf,
    weights: PathBuf,
    encoder_config: PathBuf,
    tokenizer: PathBuf,
}

/// System-1 decision model runtime.
pub struct Agent {
    model: DecisionModel,
    tok: LayaTokenizer,
    max_len: usize,
    head_max_len: usize,
    temperature: [f64; 3],
    temperature_by_options: std::collections::HashMap<String, f64>,
}

impl Agent {
    /// Load a Laya checkpoint from a local directory or a Hugging Face repo id.
    pub fn load(model_id_or_path: &str, opts: LoadOptions) -> Result<Agent> {
        let files = resolve_files(model_id_or_path, &opts)?;

        let cfg: Value = serde_json::from_slice(&std::fs::read(&files.config)?)?;
        let encoder_config: Value = serde_json::from_slice(&std::fs::read(&files.encoder_config)?)?;

        let max_len = cfg.get("max_len").and_then(|v| v.as_u64()).unwrap_or(512) as usize;
        let head_max_len = cfg
            .get("head_max_len")
            .and_then(|v| v.as_u64())
            .unwrap_or(192) as usize;
        let head_layers = cfg.get("head_layers").and_then(|v| v.as_u64()).unwrap_or(2) as usize;
        let n_act = cfg
            .get("act_costs")
            .and_then(|v| v.as_object())
            .map(|m| m.len())
            .unwrap_or(0)
            + 1;

        let temperature = load_temperature(&cfg);
        let temperature_by_options = load_temperature_by_options(&cfg);

        let device = resolve_device(opts.device.as_deref());
        let model =
            DecisionModel::load(&files.weights, &encoder_config, head_layers, n_act, device)?;
        let tok = LayaTokenizer::from_file(&files.tokenizer)?;

        Ok(Agent {
            model,
            tok,
            max_len,
            head_max_len,
            temperature,
            temperature_by_options,
        })
    }

    /// Evaluate typed questions over one state in a single forward pass.
    pub fn system_one(&self, state: &State, questions: &Questions) -> Result<SystemOneResult> {
        Ok(self
            .predict_batch(std::slice::from_ref(state), questions, None)?
            .pop()
            .expect("one state → one result"))
    }

    /// Alias for [`Agent::system_one`].
    pub fn predict(&self, state: &State, questions: &Questions) -> Result<SystemOneResult> {
        self.system_one(state, questions)
    }

    /// Evaluate the same questions over many states, sharing forward passes.
    pub fn predict_batch(
        &self,
        states: &[State],
        questions: &Questions,
        batch_size: Option<usize>,
    ) -> Result<Vec<SystemOneResult>> {
        if states.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<String> = questions.keys().cloned().collect();
        if ids.is_empty() {
            return Ok(states
                .iter()
                .map(|_| SystemOneResult {
                    model: "laya-rl-agent".to_string(),
                    answers: IndexMap::new(),
                    input_tokens: 0,
                    output_tokens: 0,
                    routing: None,
                })
                .collect());
        }

        // Validate + normalize each question once (state-independent).
        let mut internal: Vec<(String, InternalQ)> = Vec::with_capacity(ids.len());
        for qid in &ids {
            let qdef = &questions[qid];
            check_question(qid, qdef)?;
            internal.push((qid.clone(), to_internal(qdef)?));
        }

        let chunk = batch_size.filter(|&b| b > 0).unwrap_or(states.len());
        let mut results = Vec::with_capacity(states.len());

        for part in states.chunks(chunk) {
            let mut per_state_items: Vec<Vec<Item>> = Vec::with_capacity(part.len());
            for st in part {
                per_state_items.push(self.encode_state(st, &internal)?);
            }
            let batch = collate_items(&per_state_items, self.tok_pad())
                .ok_or_else(|| LayaError::Model("empty batch".to_string()))?;
            let (logits, act) = self.model.forward(&batch)?;

            let mut row = 0usize;
            for items in &per_state_items {
                let nrows = items.len();
                let n_tokens: usize = (row..row + nrows)
                    .map(|r| {
                        batch.attention_mask[r]
                            .iter()
                            .map(|&x| x as usize)
                            .sum::<usize>()
                    })
                    .sum();
                let answers = self.decode(&logits, &act, items, &internal, row);
                results.push(SystemOneResult {
                    model: "laya-rl-agent".to_string(),
                    answers,
                    input_tokens: n_tokens,
                    output_tokens: 0,
                    routing: None,
                });
                row += nrows;
            }
        }
        Ok(results)
    }

    fn tok_pad(&self) -> u32 {
        use crate::common::Tokenizer;
        self.tok.pad_id()
    }

    /// Mean-pooled encoder embeddings for a list of texts (`[n, hidden_size]`).
    ///
    /// Tokenizes with special tokens, truncates to `max_length`, runs the checkpoint encoder in
    /// batches of `batch_size`, and mean-pools over non-padding tokens. Backs
    /// [`crate::shortlist::embed_fn_from_agent`]; a dedicated bi-encoder will usually shortlist
    /// better, but this needs only the loaded checkpoint.
    pub fn embed(
        &self,
        texts: &[String],
        max_length: usize,
        batch_size: usize,
    ) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let pad = self.tok_pad();
        let mut out = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(batch_size.max(1)) {
            let rows: Vec<Vec<u32>> = chunk
                .iter()
                .map(|t| self.tok.encode_with_special(t, max_length))
                .collect();
            let l = rows.iter().map(|r| r.len()).max().unwrap_or(0).max(1);
            let mut ids = Vec::with_capacity(rows.len());
            let mut att = Vec::with_capacity(rows.len());
            for r in &rows {
                let mut row = r.clone();
                let mut a = vec![1u32; row.len()];
                row.resize(l, pad);
                a.resize(l, 0);
                ids.push(row);
                att.push(a);
            }
            out.extend(self.model.embed(&ids, &att)?);
        }
        Ok(out)
    }

    fn encode_state(&self, state: &State, internal: &[(String, InternalQ)]) -> Result<Vec<Item>> {
        let truncate_left = state.is_array();
        let mut items = Vec::with_capacity(internal.len());
        for (qid, q) in internal {
            let (seq, markers) = build_sequence(
                &self.tok,
                state,
                q,
                self.max_len,
                self.head_max_len,
                None,
                truncate_left,
            )?;
            if markers.len() != render_options(q)?.len() {
                return Err(LayaError::InvalidQuestion(format!(
                    "question {:?} options exceed head_max_len={}",
                    qid, self.head_max_len
                )));
            }
            items.push(Item {
                ids: seq,
                markers,
                qtype: q.t.index(),
            });
        }
        Ok(items)
    }

    fn decode(
        &self,
        logits: &[Vec<f32>],
        act: &[Vec<f32>],
        items: &[Item],
        internal: &[(String, InternalQ)],
        offset: usize,
    ) -> IndexMap<String, Value> {
        let mut answers = IndexMap::new();
        for (j, (qid, q)) in internal.iter().enumerate() {
            let r = offset + j;
            let k = items[j].markers.len();
            let qt = q.t.index();
            let t_scale = self
                .temperature_by_options
                .get(&temp_bucket(qt, k))
                .copied()
                .unwrap_or(self.temperature[qt]);

            let z: Vec<f64> = logits[r][..k].iter().map(|&v| v as f64 / t_scale).collect();
            let p = softmax(&z);
            let conf = round4(confidence_from_probs(&p, k));
            // `confidence` means one thing for `noul` (max(p)) and another for `choice`/`score`
            // (normalized entropy), and only the first is the quantity temperature scaling fits and
            // ECE measures. Report both: `answer_confidence` is the calibrated one on every type, so
            // a caller can gate across question types on a single number.
            let ans_conf = round4(answer_confidence(&p, k));

            let act_p = softmax(&act[r].iter().map(|&v| v as f64).collect::<Vec<_>>());
            let act_probability = round4(act_p.first().copied().unwrap_or(0.0));
            let action = json!({ "act_probability": act_probability });

            let answer = match q.t {
                QType::Choice => {
                    let keys: Vec<String> = q
                        .crit
                        .as_object()
                        .map(|m| m.keys().cloned().collect())
                        .unwrap_or_default();
                    let best = argmax(&p);
                    let probs: Map<String, Value> = keys
                        .iter()
                        .zip(p.iter())
                        .map(|(kk, &v)| (kk.clone(), json!(round4(v))))
                        .collect();
                    json!({
                        "type": "choice",
                        "choice": keys.get(best).cloned().unwrap_or_default(),
                        "probabilities": Value::Object(probs),
                        "confidence": conf,
                        "answer_confidence": ans_conf,
                        "action": action,
                    })
                }
                QType::Score => {
                    let exp: f64 = p.iter().enumerate().map(|(i, &v)| i as f64 * v).sum();
                    let legend: Map<String, Value> = q
                        .crit
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .enumerate()
                                .map(|(i, c)| (i.to_string(), c.clone()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let probs: Map<String, Value> = p
                        .iter()
                        .enumerate()
                        .map(|(i, &v)| (i.to_string(), json!(round4(v))))
                        .collect();
                    json!({
                        "type": "score",
                        "score": round4(exp),
                        "legend": Value::Object(legend),
                        "probabilities": Value::Object(probs),
                        "confidence": conf,
                        "answer_confidence": ans_conf,
                        "action": action,
                    })
                }
                QType::Noul => {
                    let p_true = p.get(1).copied().unwrap_or(0.0);
                    json!({
                        "type": "noul",
                        "noul": round4(p_true),
                        "confidence": round4(p_true.max(1.0 - p_true)),
                        // identical here: over two options max(p_true, 1 - p_true) is max(p)
                        "answer_confidence": ans_conf,
                        "action": action,
                    })
                }
            };
            answers.insert(qid.clone(), answer);
        }
        answers
    }
}

fn argmax(p: &[f64]) -> usize {
    let mut best = 0usize;
    for i in 1..p.len() {
        if p[i] > p[best] {
            best = i;
        }
    }
    best
}

fn load_temperature(cfg: &Value) -> [f64; 3] {
    let arr = cfg.get("temperature").and_then(|v| v.as_array());
    let mut out = [1.0; 3];
    if let Some(a) = arr {
        for (i, slot) in out.iter_mut().enumerate() {
            if let Some(v) = a.get(i) {
                *slot = clamp_temperature(v);
            }
        }
    }
    out
}

fn load_temperature_by_options(cfg: &Value) -> std::collections::HashMap<String, f64> {
    cfg.get("temperature_by_options")
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), clamp_temperature(v)))
                .collect()
        })
        .unwrap_or_default()
}

fn resolve_device(device: Option<&str>) -> Device {
    match device.map(|d| d.trim().to_lowercase()).as_deref() {
        Some("metal") => {
            #[cfg(feature = "metal")]
            {
                match Device::new_metal(0) {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!(
                            "laya: Metal requested but unavailable ({e}); falling back to CPU."
                        );
                        Device::Cpu
                    }
                }
            }
            #[cfg(not(feature = "metal"))]
            {
                eprintln!(
                    "laya: device=\"metal\" requested but the crate was built without the `metal` \
                     feature; using CPU. Rebuild with --features metal."
                );
                Device::Cpu
            }
        }
        // "cuda" and other accelerators are follow-ups; everything else runs on CPU.
        _ => Device::Cpu,
    }
}

/// Validate a question definition, matching `Agent._check_question`.
fn check_question(qid: &str, qdef: &Value) -> Result<()> {
    let obj = match qdef {
        Value::Object(m) => m,
        other => {
            let kind = match other {
                Value::Array(_) => "list",
                Value::Null => "NoneType",
                Value::String(_) => "str",
                Value::Bool(_) => "bool",
                Value::Number(_) => "number",
                Value::Object(_) => unreachable!(),
            };
            return Err(LayaError::InvalidQuestion(format!(
                "question {:?}: definition must be a dict, got {}",
                qid, kind
            )));
        }
    };
    let t = obj.get("type").and_then(|v| v.as_str());
    let t = match t.and_then(QType::from_name) {
        Some(t) => t,
        None => {
            return Err(LayaError::InvalidQuestion(format!(
                "question {:?}: unknown type {:?}; use one of [\"choice\", \"noul\", \"score\"]",
                qid,
                obj.get("type").unwrap_or(&Value::Null)
            )))
        }
    };
    if !obj.contains_key("instructions") {
        return Err(LayaError::InvalidQuestion(format!(
            "question {:?}: no 'instructions'; add the text the model should answer",
            qid
        )));
    }
    let crit = obj.get("criteria");
    match t {
        QType::Choice => {
            let ok = matches!(crit, Some(Value::Object(_)) | Some(Value::Array(_)));
            if !ok {
                return Err(LayaError::InvalidQuestion(format!(
                    "question {:?}: a choice question takes 'criteria' as a dict of label -> description, or a list of labels",
                    qid
                )));
            }
            let empty = match crit {
                Some(Value::Object(m)) => m.is_empty(),
                Some(Value::Array(a)) => a.is_empty(),
                _ => true,
            };
            if empty {
                return Err(LayaError::InvalidQuestion(format!(
                    "question {:?}: a choice question needs at least one criterion",
                    qid
                )));
            }
            // A list label is used as the answer key when a list of labels is normalised (a dict's
            // keys are already strings), so a nested array/object label has no meaning here — it is
            // rendered as option text. Reject it as a named caller error (a 422 over HTTP, not the
            // opaque 500 an inscrutable failure three frames down would become) (upstream #425).
            if let Some(Value::Array(a)) = crit {
                if let Some(i) = a
                    .iter()
                    .position(|v| matches!(v, Value::Array(_) | Value::Object(_)))
                {
                    // "array" and "object" both start with a vowel → "an".
                    let kind = if a[i].is_array() { "array" } else { "object" };
                    return Err(LayaError::InvalidQuestion(format!(
                        "question {:?}: choice label {} is an {}; a label is rendered as option \
                         text and used as the answer key, so it must be a scalar (a string, number \
                         or null), got {}",
                        qid,
                        i,
                        kind,
                        crate::common::py_json(&a[i])
                    )));
                }
            }
        }
        QType::Score => {
            let arr = match crit {
                Some(Value::Array(a)) => a,
                _ => {
                    return Err(LayaError::InvalidQuestion(format!(
                        "question {:?}: a score question takes 'criteria' as a list of level descriptions, index 0 first",
                        qid
                    )))
                }
            };
            if arr.is_empty() {
                return Err(LayaError::InvalidQuestion(format!(
                    "question {:?}: a score question needs at least one level",
                    qid
                )));
            }
            // A null level would silently drop that description; reject it and name the index so
            // every level gets described, index 0 first (upstream: reject a null score level).
            if let Some(idx) = arr.iter().position(|v| v.is_null()) {
                return Err(LayaError::InvalidQuestion(format!(
                    "question {:?}: score level {} is null; give every level a description, index 0 first",
                    qid, idx
                )));
            }
        }
        QType::Noul => {
            if let Some(c) = crit {
                if !c.is_null() && !c.is_object() {
                    return Err(LayaError::InvalidQuestion(format!(
                        "question {:?}: a noul question takes 'criteria' as a dict with optional 'true'/'false' descriptions, or omits it",
                        qid
                    )));
                }
                // `render_options` reads only `false`/`true` out by name, so a dict keyed any other
                // way is not a noul description at all — it used to be silently dropped and replaced
                // with the defaults (#156). A noul is a boolean question, so those are its only keys.
                if let Some(map) = c.as_object() {
                    let mut keys: Vec<String> = map.keys().map(|k| k.to_lowercase()).collect();
                    if keys.iter().any(|k| k != "true" && k != "false") {
                        keys.sort();
                        let shown: Vec<Value> = keys.into_iter().map(Value::String).collect();
                        return Err(LayaError::InvalidQuestion(format!(
                            "question {:?}: a noul question takes 'criteria' keyed only 'true'/'false' (either or both, and omitted is fine), got {}. Those keys are the option texts the model reads; any other key was silently dropped and replaced with the defaults. If you want the answer worded differently, keep 'criteria' keyed 'true'/'false' and set 'labels' instead.",
                            qid,
                            Value::Array(shown)
                        )));
                    }
                }
            }
        }
    }
    if obj.contains_key("labels") {
        if t != QType::Noul {
            return Err(LayaError::InvalidQuestion(format!(
                "question {:?}: 'labels' is only supported for noul questions",
                qid
            )));
        }
        crate::common::resolve_noul_labels(obj.get("labels"))
            .map_err(|e| LayaError::InvalidQuestion(format!("question {:?}: {}", qid, e)))?;
    }
    Ok(())
}

/// Normalize a validated question into the internal form.
fn to_internal(qdef: &Value) -> Result<InternalQ> {
    let obj = qdef.as_object().expect("validated as object");
    let t = QType::from_name(obj.get("type").and_then(|v| v.as_str()).unwrap_or("")).unwrap();
    let mut crit = obj.get("criteria").cloned().unwrap_or(Value::Null);
    match t {
        QType::Choice => {
            if let Value::Array(a) = &crit {
                let mut m = Map::new();
                for c in a {
                    let key = c
                        .as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| crate::common::py_json(c));
                    m.insert(key, Value::Null);
                }
                crit = Value::Object(m);
            }
        }
        QType::Noul => {
            if let Value::Object(m) = &crit {
                let lowered: Map<String, Value> = m
                    .iter()
                    .map(|(k, v)| (k.to_lowercase(), v.clone()))
                    .collect();
                crit = Value::Object(lowered);
            }
        }
        QType::Score => {}
    }
    let ins = match obj.get("instructions") {
        Some(Value::String(s)) => s.clone(),
        Some(other) => crate::common::py_json(other),
        None => String::new(),
    };
    let labels = obj.get("labels").cloned();
    Ok(InternalQ {
        t,
        ins,
        crit,
        labels,
    })
}

/// Resolve the four checkpoint files from a local dir or a Hugging Face repo id.
fn resolve_files(model_id_or_path: &str, opts: &LoadOptions) -> Result<CheckpointFiles> {
    let base = Path::new(model_id_or_path);
    if base.exists() {
        let dir = match &opts.subfolder {
            Some(sub) => base.join(sub),
            None => base.to_path_buf(),
        };
        let config = require(dir.join("rl_agent_config.json"))?;
        let weights = require(dir.join("model.safetensors"))?;
        let encoder_config = if dir.join("encoder/config.json").exists() {
            dir.join("encoder/config.json")
        } else {
            require(dir.join("config.json"))?
        };
        let tokenizer = if dir.join("tokenizer/tokenizer.json").exists() {
            dir.join("tokenizer/tokenizer.json")
        } else {
            require(dir.join("tokenizer.json"))?
        };
        return Ok(CheckpointFiles {
            config,
            weights,
            encoder_config,
            tokenizer,
        });
    }
    if model_id_or_path.starts_with('/')
        || model_id_or_path.starts_with("./")
        || model_id_or_path.starts_with("../")
    {
        return Err(LayaError::NotFound(format!(
            "Local model path not found: {:?}.",
            model_id_or_path
        )));
    }
    download_files(model_id_or_path, opts)
}

fn require(p: PathBuf) -> Result<PathBuf> {
    if p.exists() {
        Ok(p)
    } else {
        Err(LayaError::NotFound(format!(
            "missing checkpoint file: {}",
            p.display()
        )))
    }
}

fn download_files(repo: &str, opts: &LoadOptions) -> Result<CheckpointFiles> {
    use hf_hub::api::sync::ApiBuilder;
    // An empty token (explicit or from an empty `HF_TOKEN`) is treated as no token, so we never
    // send an empty `Bearer` header to the hub (parity with upstream `token or HF_TOKEN or None`).
    let token = opts
        .token
        .clone()
        .or_else(|| std::env::var("HF_TOKEN").ok())
        .filter(|t| !t.is_empty());
    let api = ApiBuilder::new()
        .with_token(token)
        .build()
        .map_err(|e| LayaError::Download(e.to_string()))?;
    let repo_api = api.model(repo.to_string());
    let prefix = match &opts.subfolder {
        Some(sub) => format!("{}/", sub),
        None => String::new(),
    };
    let get = |name: &str| -> Result<PathBuf> {
        repo_api
            .get(&format!("{}{}", prefix, name))
            .map_err(|e| LayaError::Download(format!("{}{}: {}", prefix, name, e)))
    };
    let config = get("rl_agent_config.json")?;
    let weights = get("model.safetensors")?;
    let encoder_config = get("encoder/config.json")?;
    let tokenizer = get("tokenizer/tokenizer.json").or_else(|_| get("tokenizer.json"))?;
    Ok(CheckpointFiles {
        config,
        weights,
        encoder_config,
        tokenizer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_null_score_level() {
        // A null level would silently drop that description; it must be rejected, naming the index.
        let q = json!({"type": "score", "instructions": "rate severity",
                       "criteria": ["low", null, "high"]});
        let msg = check_question("severity", &q).unwrap_err().to_string();
        assert!(msg.contains("score level 1 is null"), "got: {msg}");
    }

    #[test]
    fn accepts_fully_described_score_levels() {
        let q = json!({"type": "score", "instructions": "rate", "criteria": ["low", "high"]});
        assert!(check_question("s", &q).is_ok());
    }

    #[test]
    fn rejects_nested_choice_label() {
        // A nested array/object label is a named caller error, not an opaque failure (#425).
        let q = json!({"type": "choice", "instructions": "pick",
                       "criteria": ["fraud", ["nested", "label"]]});
        let msg = check_question("kind", &q).unwrap_err().to_string();
        assert!(msg.contains("choice label 1 is an array"), "got: {msg}");

        let q = json!({"type": "choice", "instructions": "pick",
                       "criteria": ["ok", {"bad": "label"}]});
        let msg = check_question("kind", &q).unwrap_err().to_string();
        assert!(msg.contains("choice label 1 is an object"), "got: {msg}");
    }

    #[test]
    fn accepts_scalar_choice_labels() {
        let q = json!({"type": "choice", "instructions": "pick", "criteria": ["a", "b", 3, null]});
        assert!(check_question("k", &q).is_ok());
    }
}
