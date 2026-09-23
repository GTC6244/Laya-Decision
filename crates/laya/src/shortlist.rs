//! Opt-in embedding shortlist for high-cardinality choice questions. Ported from
//! `laya/shortlist.py`. The ranking core (`shortlist_choice`, `predict_shortlist`) takes a
//! caller-supplied embedding function `embed_fn: Fn(&[String]) -> Vec<Vec<f64>>`;
//! [`embed_fn_from_agent`] builds one from a loaded checkpoint's encoder.

use crate::agent::Agent;
use crate::common::{render_options, serialize_state, InternalQ, QType};
use crate::error::{LayaError, Result};
use crate::Questions;
use serde_json::{json, Map, Value};

pub const DEFAULT_SHORTLIST_K: usize = 20;
/// Defaults matching `embed_fn_from_agent` in `laya/shortlist.py`.
pub const DEFAULT_EMBED_MAX_LENGTH: usize = 512;
pub const DEFAULT_EMBED_BATCH_SIZE: usize = 32;

/// Build an `embed_fn` that mean-pools the checkpoint encoder already loaded on `agent`.
///
/// The returned closure embeds a list of strings for [`shortlist_choice`]/[`predict_shortlist`].
/// A dedicated bi-encoder passed directly as `embed_fn` will usually shortlist better; this helper
/// is for callers who only have the Laya checkpoint in memory. On an embedding failure it returns
/// an empty vector, which surfaces as a clear length-mismatch error from the ranker.
pub fn embed_fn_from_agent(
    agent: &Agent,
    max_length: usize,
    batch_size: usize,
) -> impl Fn(&[String]) -> Vec<Vec<f64>> + '_ {
    move |texts: &[String]| match agent.embed(texts, max_length, batch_size) {
        Ok(rows) => rows
            .into_iter()
            .map(|row| row.into_iter().map(|x| x as f64).collect())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Return the top-`k` choice labels for `state`, ranked by cosine similarity of `embed_fn`
/// vectors (query first, then one per option in criteria order). When `k >= n`, every label is
/// returned in original order and `embed_fn` is not called. Ties keep the earlier label.
pub fn shortlist_choice<F>(
    state: &Value,
    criteria: &Value,
    embed_fn: F,
    k: usize,
    instructions: Option<&str>,
) -> Result<Vec<String>>
where
    F: Fn(&[String]) -> Vec<Vec<f64>>,
{
    let (labels, _scores, _passthrough, _n) = rank(state, criteria, embed_fn, k, instructions)?;
    Ok(labels)
}

/// Shortlist each choice question, then run the agent once on the reduced question set.
/// Returns the result JSON (Jev-shaped) with an added `shortlist` entry.
pub fn predict_shortlist<F>(
    agent: &Agent,
    state: &Value,
    questions: &Questions,
    embed_fn: F,
    k: usize,
) -> Result<Value>
where
    F: Fn(&[String]) -> Vec<Vec<f64>>,
{
    let checked = check_k(k)?;
    let mut reduced: Questions = Questions::new();
    let mut meta = Map::new();

    for (qid, qdef) in questions {
        let is_choice = qdef.get("type").and_then(|v| v.as_str()) == Some("choice");
        if !qdef.is_object() || !is_choice {
            reduced.insert(qid.clone(), qdef.clone());
            continue;
        }
        let criteria = qdef.get("criteria").ok_or_else(|| {
            LayaError::InvalidQuestion(format!("question {:?} is a choice but has no criteria", qid))
        })?;
        let instructions = qdef.get("instructions").and_then(|v| v.as_str());
        let (labels, scores, passthrough, n) =
            rank(state, criteria, &embed_fn, checked, instructions)?;
        meta.insert(
            qid.clone(),
            json!({
                "labels": labels,
                "scores": scores,
                "k": checked,
                "n": n,
                "passthrough": passthrough,
            }),
        );
        if passthrough {
            reduced.insert(qid.clone(), qdef.clone());
            continue;
        }
        let mut updated = qdef.clone();
        updated["criteria"] = subset_criteria(criteria, &labels);
        reduced.insert(qid.clone(), updated);
    }

    let result = agent.system_one(state, &reduced)?;
    let mut out = result.to_json();
    out.as_object_mut()
        .unwrap()
        .insert("shortlist".to_string(), Value::Object(meta));
    Ok(out)
}

fn rank<F>(
    state: &Value,
    criteria: &Value,
    embed_fn: F,
    k: usize,
    instructions: Option<&str>,
) -> Result<(Vec<String>, Option<Vec<f64>>, bool, usize)>
where
    F: Fn(&[String]) -> Vec<Vec<f64>>,
{
    let checked = check_k(k)?;
    let keys = criteria_keys(criteria)?;
    let n = keys.len();
    if checked >= n {
        return Ok((keys, None, true, n));
    }
    let query = query_text(state, instructions);
    let option_texts = option_texts(criteria)?;
    let mut texts = Vec::with_capacity(1 + option_texts.len());
    texts.push(query);
    texts.extend(option_texts);
    let matrix = embed_fn(&texts);
    if matrix.len() != texts.len() {
        return Err(LayaError::InvalidQuestion(format!(
            "embed_fn must return {} vectors, got {}",
            texts.len(),
            matrix.len()
        )));
    }
    let sims = cosine(&matrix[0], &matrix[1..]);
    // stable sort by descending similarity (ties keep earlier index)
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        sims[b]
            .partial_cmp(&sims[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    order.truncate(checked);
    let labels = order.iter().map(|&i| keys[i].clone()).collect();
    let scores = order.iter().map(|&i| sims[i]).collect();
    Ok((labels, Some(scores), false, n))
}

fn check_k(k: usize) -> Result<usize> {
    if k < 1 {
        return Err(LayaError::InvalidQuestion(
            "k must be a positive integer".to_string(),
        ));
    }
    Ok(k)
}

fn criteria_keys(criteria: &Value) -> Result<Vec<String>> {
    let keys: Vec<String> = match criteria {
        Value::Object(m) => m.keys().cloned().collect(),
        Value::Array(a) => a
            .iter()
            .map(|v| v.as_str().map(|s| s.to_string()).unwrap_or_else(|| crate::common::py_json(v)))
            .collect(),
        _ => {
            return Err(LayaError::InvalidQuestion(
                "choice criteria must be a dict or list".to_string(),
            ))
        }
    };
    if keys.is_empty() {
        return Err(LayaError::InvalidQuestion(
            "choice criteria must contain at least one option".to_string(),
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for k in &keys {
        if !seen.insert(k.clone()) {
            return Err(LayaError::InvalidQuestion(format!(
                "choice criteria label {:?} is duplicated",
                k
            )));
        }
    }
    Ok(keys)
}

fn option_texts(criteria: &Value) -> Result<Vec<String>> {
    let q = InternalQ {
        t: QType::Choice,
        ins: String::new(),
        crit: normalize_choice_crit(criteria),
        labels: None,
    };
    render_options(&q)
}

fn normalize_choice_crit(criteria: &Value) -> Value {
    match criteria {
        Value::Array(a) => {
            let mut m = Map::new();
            for c in a {
                let key = c
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| crate::common::py_json(c));
                m.insert(key, Value::Null);
            }
            Value::Object(m)
        }
        other => other.clone(),
    }
}

fn query_text(state: &Value, instructions: Option<&str>) -> String {
    let body = serialize_state(state);
    match instructions {
        None | Some("") => body,
        Some(ins) => format!("{}\n{}", ins, body),
    }
}

fn subset_criteria(criteria: &Value, labels: &[String]) -> Value {
    match criteria {
        Value::Object(m) => {
            let mut out = Map::new();
            for l in labels {
                if let Some(v) = m.get(l) {
                    out.insert(l.clone(), v.clone());
                }
            }
            Value::Object(out)
        }
        _ => Value::Array(labels.iter().map(|l| json!(l)).collect()),
    }
}

fn cosine(query: &[f64], docs: &[Vec<f64>]) -> Vec<f64> {
    let qn = query.iter().map(|v| v * v).sum::<f64>().sqrt();
    if qn == 0.0 || docs.is_empty() {
        return vec![0.0; docs.len()];
    }
    docs.iter()
        .map(|d| {
            let dn = d.iter().map(|v| v * v).sum::<f64>().sqrt();
            let denom = dn * qn;
            if denom > 0.0 {
                let dot: f64 = d.iter().zip(query).map(|(a, b)| a * b).sum();
                (dot / denom).clamp(-1.0, 1.0)
            } else {
                0.0
            }
        })
        .collect()
}
