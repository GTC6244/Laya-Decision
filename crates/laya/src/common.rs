//! Core model-independent logic: state serialization (Python `json.dumps` parity),
//! option rendering, token-sequence construction, collation, and calibration helpers.
//!
//! Ported from `laya/common.py` (source of truth) and mirrored against `laya-ts/src/common.ts`.

use crate::error::{LayaError, Result};
use serde_json::Value;

/// The three question types. Numeric indices match Python `QTYPES`
/// (`choice=0, score=1, noul=2`) — they select the model's type embedding row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QType {
    Choice,
    Score,
    Noul,
}

impl QType {
    pub fn index(self) -> usize {
        match self {
            QType::Choice => 0,
            QType::Score => 1,
            QType::Noul => 2,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            QType::Choice => "choice",
            QType::Score => "score",
            QType::Noul => "noul",
        }
    }

    pub fn from_name(s: &str) -> Option<QType> {
        match s {
            "choice" => Some(QType::Choice),
            "score" => Some(QType::Score),
            "noul" => Some(QType::Noul),
            _ => None,
        }
    }

    pub fn from_index(i: usize) -> Option<QType> {
        match i {
            0 => Some(QType::Choice),
            1 => Some(QType::Score),
            2 => Some(QType::Noul),
            _ => None,
        }
    }
}

/// A validated + normalized question, ready for sequence building and decoding.
/// `crit` is kept as a JSON value in the shape the type expects (choice: object,
/// score: array, noul: object-or-null), matching the normalization in `Agent::to_internal`.
#[derive(Debug, Clone)]
pub struct InternalQ {
    pub t: QType,
    pub ins: String,
    pub crit: Value,
    pub labels: Option<Value>,
}

/// Abstraction over a checkpoint tokenizer. Mirrors `TokenizerLike` in laya-ts.
/// `encode` corresponds to HF `tokenizer(text, add_special_tokens=False)["input_ids"]`.
pub trait Tokenizer {
    fn cls_id(&self) -> u32;
    fn sep_id(&self) -> u32;
    fn mask_id(&self) -> u32;
    fn pad_id(&self) -> u32;
    fn mask_token(&self) -> &str;
    fn encode(&self, text: &str) -> Vec<u32>;
}

const DEFAULT_NOUL_FALSE: &str = "false";
const DEFAULT_NOUL_TRUE: &str = "true";

/// Replica of Python `json.dumps(value, ensure_ascii=False, separators=(", ", ": "))`.
///
/// This differs from `serde_json::to_string` (which uses no spaces after `,`/`:`); the exact
/// spacing matters because serialized states/criteria are tokenized, so a spacing difference
/// changes token ids. Mirrors `pyJson` in `laya-ts/src/common.ts`.
pub fn py_json(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => {
            if *b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Value::Number(n) => n.to_string(),
        // serde_json escapes control chars / `"` / `\` and keeps non-ASCII raw — same as
        // Python `ensure_ascii=False`.
        Value::String(_) => serde_json::to_string(v).unwrap_or_else(|_| "\"\"".to_string()),
        Value::Array(a) => {
            let parts: Vec<String> = a.iter().map(py_json).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Object(m) => {
            let parts: Vec<String> = m
                .iter()
                .map(|(k, val)| {
                    let key = serde_json::to_string(&Value::String(k.clone()))
                        .unwrap_or_else(|_| "\"\"".to_string());
                    format!("{}: {}", key, py_json(val))
                })
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

/// A state as the model sees it: raw strings pass through, everything else becomes compact JSON.
pub fn serialize_state(state: &Value) -> String {
    match state {
        Value::String(s) => s.clone(),
        other => py_json(other),
    }
}

/// Render one criterion value as text (strings pass through, structured values become JSON).
pub fn render_criterion(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => py_json(other),
    }
}

/// True when a criterion value means "no description" (only `null`/`""`; `0`/`false` are real).
fn is_empty_criterion(v: Option<&Value>) -> bool {
    match v {
        None => true,
        Some(Value::Null) => true,
        Some(Value::String(s)) if s.is_empty() => true,
        _ => false,
    }
}

/// Resolve noul labels to `(false_label, true_label)`, validating the mapping.
pub fn resolve_noul_labels(labels: Option<&Value>) -> Result<(String, String)> {
    let err = || {
        LayaError::InvalidQuestion(
            "noul labels must map exactly 'false' and 'true' to distinct non-empty strings"
                .to_string(),
        )
    };
    let (f, t) = match labels {
        None | Some(Value::Null) => {
            return Ok((DEFAULT_NOUL_FALSE.to_string(), DEFAULT_NOUL_TRUE.to_string()))
        }
        Some(Value::Object(m)) => {
            let keys: std::collections::BTreeSet<&str> = m.keys().map(|s| s.as_str()).collect();
            let want: std::collections::BTreeSet<&str> = ["false", "true"].into_iter().collect();
            if keys != want {
                return Err(err());
            }
            (m.get("false"), m.get("true"))
        }
        _ => return Err(err()),
    };
    let (f, t) = match (f, t) {
        (Some(Value::String(f)), Some(Value::String(t))) => (f, t),
        _ => return Err(err()),
    };
    let f = f.trim();
    let t = t.trim();
    if f.is_empty() || t.is_empty() || f == t {
        return Err(err());
    }
    Ok((f.to_string(), t.to_string()))
}

/// Render option texts in label-index order. Noul semantic order is always `[false, true]`.
pub fn render_options(q: &InternalQ) -> Result<Vec<String>> {
    if q.t != QType::Noul && q.labels.is_some() {
        return Err(LayaError::InvalidQuestion(
            "labels is only supported for noul questions".to_string(),
        ));
    }
    match q.t {
        QType::Choice => {
            let obj = q.crit.as_object().ok_or_else(|| {
                LayaError::InvalidQuestion("choice criteria must be an object".to_string())
            })?;
            Ok(obj
                .iter()
                .map(|(k, v)| {
                    if is_empty_criterion(Some(v)) {
                        k.clone()
                    } else {
                        format!("{}: {}", k, render_criterion(v))
                    }
                })
                .collect())
        }
        QType::Score => {
            let arr = q.crit.as_array().ok_or_else(|| {
                LayaError::InvalidQuestion("score criteria must be a list".to_string())
            })?;
            Ok(arr
                .iter()
                .enumerate()
                .map(|(i, c)| format!("level {}: {}", i, render_criterion(c)))
                .collect())
        }
        QType::Noul => {
            let (false_label, true_label) = resolve_noul_labels(q.labels.as_ref())?;
            let empty = serde_json::Map::new();
            let crit = q.crit.as_object().unwrap_or(&empty);
            let false_crit = crit.get("false");
            let true_crit = crit.get("true");
            let false_text = if is_empty_criterion(false_crit) {
                "no, the statement does not hold".to_string()
            } else {
                render_criterion(false_crit.unwrap())
            };
            let true_text = if is_empty_criterion(true_crit) {
                "yes, the statement holds".to_string()
            } else {
                render_criterion(true_crit.unwrap())
            };
            Ok(vec![
                format!("{}: {}", false_label, false_text),
                format!("{}: {}", true_label, true_text),
            ])
        }
    }
}

/// Build one token sequence + option marker positions.
///
/// Format: `[CLS] <type> question: <instructions> [SEP] [MASK] opt0 [MASK] opt1 ... [SEP] state [SEP]`.
/// Ported verbatim from `build_sequence` in `laya/common.py`.
pub fn build_sequence<T: Tokenizer + ?Sized>(
    tok: &T,
    state: &Value,
    q: &InternalQ,
    max_len: usize,
    head_max_len: usize,
    option_order: Option<&[usize]>,
    truncate_left: bool,
) -> Result<(Vec<u32>, Vec<usize>)> {
    let mask_tok = tok.mask_token();
    let opts = render_options(q)?;
    let order: Vec<usize> = match option_order {
        Some(o) => o.to_vec(),
        None => (0..opts.len()).collect(),
    };

    let ins = q.ins.replace(mask_tok, " ");
    let head_text = format!("{} question: {}", q.t.name(), ins);
    let mut head_ids = tok.encode(&head_text);

    let mut opt_ids: Vec<Vec<u32>> = Vec::with_capacity(order.len());
    for &i in &order {
        let mut o = vec![tok.mask_id()];
        let enc = tok.encode(&format!(" {}", opts[i].replace(mask_tok, " ")));
        o.extend(enc.into_iter().take(48));
        opt_ids.push(o);
    }

    let sum_opt = |v: &[Vec<u32>]| -> usize { v.iter().map(|o| o.len()).sum() };
    let mut opt_budget: isize = head_max_len as isize - sum_opt(&opt_ids) as isize;
    if opt_budget < 16 {
        let denom = std::cmp::max(1, opt_ids.len()) as isize;
        let per = std::cmp::max(4, (head_max_len as isize - 16) / denom) as usize;
        for o in opt_ids.iter_mut() {
            o.truncate(per);
        }
        opt_budget = head_max_len as isize - sum_opt(&opt_ids) as isize;
    }
    let head_keep = opt_budget.max(8) as usize;
    head_ids.truncate(head_keep.min(head_ids.len()));

    let mut ids: Vec<u32> = Vec::new();
    ids.push(tok.cls_id());
    ids.extend_from_slice(&head_ids);
    ids.push(tok.sep_id());

    let mut markers: Vec<usize> = Vec::with_capacity(opt_ids.len());
    for o in &opt_ids {
        markers.push(ids.len());
        ids.extend_from_slice(o);
    }
    ids.push(tok.sep_id());

    let room = (max_len as isize - ids.len() as isize - 1).max(0) as usize;
    let st_all = tok.encode(&serialize_state(state).replace(mask_tok, " "));
    let st: Vec<u32> = if truncate_left {
        let start = st_all.len().saturating_sub(room);
        st_all[start..].to_vec()
    } else {
        st_all.into_iter().take(room).collect()
    };
    ids.extend_from_slice(&st);
    ids.push(tok.sep_id());
    ids.truncate(max_len);

    let markers = markers.into_iter().filter(|&m| m < max_len).collect();
    Ok((ids, markers))
}

/// Round to `ndigits` decimals using round-half-to-even, matching Python's built-in `round`.
pub fn round_half_even(x: f64, ndigits: i32) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let factor = 10f64.powi(ndigits);
    let scaled = x * factor;
    let floor = scaled.floor();
    let diff = scaled - floor;
    let rounded = if (diff - 0.5).abs() < 1e-9 {
        // exact half → round to even
        if (floor as i64) % 2 == 0 {
            floor
        } else {
            floor + 1.0
        }
    } else {
        scaled.round()
    };
    rounded / factor
}

/// Round to 4 decimals like Python `round(x, 4)` (the precision Laya publishes).
pub fn round4(x: f64) -> f64 {
    round_half_even(x, 4)
}

/// Numerically stable softmax.
pub fn softmax(z: &[f64]) -> Vec<f64> {
    if z.is_empty() {
        return Vec::new();
    }
    let m = z.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let exps: Vec<f64> = z.iter().map(|&v| (v - m).exp()).collect();
    let sum: f64 = exps.iter().sum();
    exps.into_iter().map(|v| v / sum).collect()
}

/// Normalized Shannon-entropy confidence: `1 - H(p)/log(k)`, clamped to `[0, 1]`.
/// `p` is expected to already hold the `k` option probabilities.
pub fn confidence_from_probs(p: &[f64], k: usize) -> f64 {
    if k < 2 {
        return 1.0;
    }
    let p = &p[..k.min(p.len())];
    let ent: f64 = -p
        .iter()
        .map(|&v| v * v.clamp(1e-12, 1.0).ln())
        .sum::<f64>();
    (1.0 - ent / (k as f64).ln()).clamp(0.0, 1.0)
}

pub const TEMP_MIN: f64 = 0.5;
pub const TEMP_MAX: f64 = 5.0;

/// A usable temperature: `t` confined to `[TEMP_MIN, TEMP_MAX]`, falling back to 1.0 if not finite.
pub fn clamp_temperature(t: &Value) -> f64 {
    let f = match t {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    };
    match f {
        Some(x) if x.is_finite() => x.clamp(TEMP_MIN, TEMP_MAX),
        _ => 1.0,
    }
}

/// Temperature bucket key, e.g. `"choice:3-5"`.
pub fn temp_bucket(qtype: usize, k: usize) -> String {
    let size = if k <= 2 {
        "2"
    } else if k <= 5 {
        "3-5"
    } else if k <= 10 {
        "6-10"
    } else {
        "11+"
    };
    let name = QType::from_index(qtype).map(|q| q.name()).unwrap_or("choice");
    format!("{}:{}", name, size)
}

/// One tokenized (state, question) pair before collation.
#[derive(Debug, Clone)]
pub struct Item {
    pub ids: Vec<u32>,
    pub markers: Vec<usize>,
    pub qtype: usize,
}

/// A padded batch ready to be turned into tensors.
#[derive(Debug, Clone)]
pub struct CollatedBatch {
    pub input_ids: Vec<Vec<u32>>,
    pub attention_mask: Vec<Vec<u32>>,
    pub marker_pos: Vec<Vec<usize>>,
    pub marker_mask: Vec<Vec<bool>>,
    pub qtype: Vec<usize>,
}

/// Collate groups of items into one padded batch. `groups` is a list of per-state item lists,
/// flattened in order (mirrors `collate_items` in `laya/common.py`). Returns `None` if empty.
pub fn collate_items(groups: &[Vec<Item>], _pad_id: u32) -> Option<CollatedBatch> {
    let items: Vec<&Item> = groups.iter().flatten().collect();
    if items.is_empty() {
        return None;
    }
    let l = items.iter().map(|it| it.ids.len()).max().unwrap();
    let kmax = items.iter().map(|it| it.markers.len()).max().unwrap();

    let mut input_ids = Vec::with_capacity(items.len());
    let mut attention_mask = Vec::with_capacity(items.len());
    let mut marker_pos = Vec::with_capacity(items.len());
    let mut marker_mask = Vec::with_capacity(items.len());
    let mut qtype = Vec::with_capacity(items.len());

    for it in &items {
        let mut ids = it.ids.clone();
        let mut att = vec![1u32; ids.len()];
        ids.resize(l, _pad_id);
        att.resize(l, 0);
        input_ids.push(ids);
        attention_mask.push(att);

        let k = it.markers.len();
        let mut mpos: Vec<usize> = it.markers.clone();
        mpos.resize(kmax, 0);
        let mut mmask: Vec<bool> = vec![true; k];
        mmask.resize(kmax, false);
        marker_pos.push(mpos);
        marker_mask.push(mmask);

        qtype.push(it.qtype);
    }

    Some(CollatedBatch {
        input_ids,
        attention_mask,
        marker_pos,
        marker_mask,
        qtype,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A deterministic fake tokenizer for structural tests (one id per whitespace word).
    struct Fake;
    impl Tokenizer for Fake {
        fn cls_id(&self) -> u32 { 1 }
        fn sep_id(&self) -> u32 { 2 }
        fn mask_id(&self) -> u32 { 3 }
        fn pad_id(&self) -> u32 { 0 }
        fn mask_token(&self) -> &str { "[MASK]" }
        fn encode(&self, text: &str) -> Vec<u32> {
            text.split_whitespace().map(|w| 100 + (w.len() as u32)).collect()
        }
    }

    #[test]
    fn py_json_matches_python_spacing() {
        // Python json.dumps({"a":1,"b":"x"}, ensure_ascii=False) -> '{"a": 1, "b": "x"}'
        let v = json!({"a": 1, "b": "x"});
        assert_eq!(py_json(&v), r#"{"a": 1, "b": "x"}"#);
        assert_eq!(py_json(&json!([1, 2, 3])), "[1, 2, 3]");
        assert_eq!(py_json(&json!("café")), "\"café\""); // non-ASCII stays raw
        assert_eq!(py_json(&json!(true)), "true");
        assert_eq!(py_json(&json!(null)), "null");
    }

    #[test]
    fn serialize_state_passes_strings_through() {
        assert_eq!(serialize_state(&json!("hello")), "hello");
        assert_eq!(serialize_state(&json!({"k": "v"})), r#"{"k": "v"}"#);
    }

    #[test]
    fn round4_half_to_even() {
        assert_eq!(round4(0.12345), 0.1234); // 0.12345 -> 0.1234 (round half to even)
        assert_eq!(round4(0.12355), 0.1236);
        assert_eq!(round4(1.0), 1.0);
    }

    #[test]
    fn render_options_per_type() {
        let choice = InternalQ {
            t: QType::Choice,
            ins: String::new(),
            crit: json!({"a": "first", "b": null, "c": ""}),
            labels: None,
        };
        assert_eq!(render_options(&choice).unwrap(), vec!["a: first", "b", "c"]);

        let score = InternalQ {
            t: QType::Score,
            ins: String::new(),
            crit: json!(["low", "high"]),
            labels: None,
        };
        assert_eq!(render_options(&score).unwrap(), vec!["level 0: low", "level 1: high"]);

        let noul = InternalQ {
            t: QType::Noul,
            ins: String::new(),
            crit: json!({"true": "yes it does", "false": "no"}),
            labels: None,
        };
        assert_eq!(
            render_options(&noul).unwrap(),
            vec!["false: no", "true: yes it does"]
        );

        let noul_default = InternalQ {
            t: QType::Noul,
            ins: String::new(),
            crit: Value::Null,
            labels: None,
        };
        assert_eq!(
            render_options(&noul_default).unwrap(),
            vec!["false: no, the statement does not hold", "true: yes, the statement holds"]
        );
    }

    #[test]
    fn temp_bucket_sizes() {
        assert_eq!(temp_bucket(0, 2), "choice:2");
        assert_eq!(temp_bucket(1, 4), "score:3-5");
        assert_eq!(temp_bucket(2, 8), "noul:6-10");
        assert_eq!(temp_bucket(0, 20), "choice:11+");
    }

    #[test]
    fn build_sequence_structure() {
        let q = InternalQ {
            t: QType::Choice,
            ins: "pick one".to_string(),
            crit: json!({"a": null, "b": null}),
            labels: None,
        };
        let (ids, markers) = build_sequence(&Fake, &json!("some state text"), &q, 512, 192, None, false).unwrap();
        assert_eq!(ids[0], 1); // [CLS]
        assert_eq!(*ids.last().unwrap(), 2); // trailing [SEP]
        assert_eq!(markers.len(), 2); // one marker per option
        // each marker points at a [MASK] id
        for &m in &markers {
            assert_eq!(ids[m], 3);
        }
    }

    #[test]
    fn confidence_bounds() {
        assert_eq!(confidence_from_probs(&[1.0], 1), 1.0); // k<2
        let uniform = confidence_from_probs(&[0.5, 0.5], 2);
        assert!(uniform.abs() < 1e-9); // max entropy -> confidence 0
        let certain = confidence_from_probs(&[1.0, 0.0], 2);
        assert!((certain - 1.0).abs() < 1e-9);
    }
}
