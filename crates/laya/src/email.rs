//! Email utilities for cleaning and structuring email inputs in laya.
//!
//! Faithful port of `laya/email.py`. The markers cover English, Portuguese and Spanish mail
//! clients: quoted-history headers, signatures, device footers and confidentiality disclaimers.
//!
//! ## Look-around re-expression
//!
//! The `regex` crate supports neither look-ahead nor look-behind, so three Python constructs are
//! re-expressed here in Rust code (each documented at its use site):
//!
//! * `_QUOTE_HEADERS` `Em (?=.*\d)…escreveu:` / `El (?=.*\d)…escribió:` — the `(?=.*\d)`
//!   look-ahead only requires a digit somewhere after the `Em `/`El ` prefix. Because that prefix
//!   contains no digit, it is equivalent to "the line contains a digit". We match the prefix-less
//!   base pattern and separately test the whole line for a `\d`.
//! * `_ATTRIBUTION_HEAD` `^\s*(On|Em|El) (?=.*\d)` — same treatment: match `^\s*(On|Em|El) ` and
//!   test the line for a digit.
//! * `_SENTENCE` `(?<=[.!?])\s+` (look-behind) — re-implemented as a manual split on any run of
//!   whitespace immediately preceded by `.`, `!` or `?` (see [`split_sentence`]).

use indexmap::IndexMap;
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

// The device/mail-app alternation, substituted twice into `DEVICE_FOOTER` just as Python does.
const DEVICE: &str = "iphone|ipad|android|ios|celular|telemóvel|móvil|galaxy|smartphone|samsung|\
tablet|outlook|yahoo|mail|e-?mail|gmail|windows";

/// Compiled regexes, matching the module-level `re.compile(...)` constants in `email.py`.
struct Regexes {
    /// `_QUOTE_HEADERS` entries WITHOUT a look-ahead (`On … wrote:`, the `----` / `____` rules,
    /// `From:` and `De: …[@<]`). The `Em`/`El` look-ahead entries are handled separately below.
    quote_plain: Vec<Regex>,
    /// `_QUOTE_HEADERS[1]`: base pattern for `Em … escreveu:` (digit checked separately).
    qh_em: Regex,
    /// `_QUOTE_HEADERS[2]`: base pattern for `El … escribió:` (digit checked separately).
    qh_el: Regex,
    /// Whole-line digit test, standing in for the `(?=.*\d)` look-aheads. Uses the `regex`
    /// crate's Unicode-aware `\d` (Decimal_Number), matching Python's `re` `\d`.
    digit: Regex,
    attribution_tail: Regex,
    /// `_ATTRIBUTION_HEAD` base pattern `^\s*(On|Em|El) ` (digit checked separately).
    attribution_head: Regex,
    header_from_name: Regex,
    header_next: Regex,
    signature_markers: Vec<Regex>,
    device_footer: Regex,
    disclaimer: Regex,
    paragraph_split: Regex,
    ws_collapse: Regex,
}

fn regexes() -> &'static Regexes {
    static RE: OnceLock<Regexes> = OnceLock::new();
    RE.get_or_init(|| {
        let device_footer = format!(
            r"(?i)^\s*((enviad[oa] (do|pelo|pela|via|desde|a partir do)( meu| minha| mi)?|sent from( my)?) ({d})( ({d}|para|for|no|na|\d+))*|(obter o|get) outlook (para|for) (ios|android))[\s.!]*$",
            d = DEVICE
        );

        Regexes {
            quote_plain: vec![
                Regex::new(r"(?i)^\s*On .{0,300}wrote:\s*$").unwrap(),
                Regex::new(r"(?i)^\s*-{2,}\s*(Original|Forwarded) Message\s*-{2,}").unwrap(),
                Regex::new(
                    r"(?i)^\s*-{2,}\s*(Mensagem (original|encaminhada)|Mensaje (original|reenviado))\s*-{2,}",
                )
                .unwrap(),
                Regex::new(r"^\s*_{8,}\s*$").unwrap(),
                Regex::new(r"(?i)^\s*From:\s.+$").unwrap(),
                Regex::new(r"(?i)^\s*De:\s.*[@<]").unwrap(),
            ],
            qh_em: Regex::new(r"(?i)^\s*Em .{0,300}escreveu:\s*$").unwrap(),
            qh_el: Regex::new(r"(?i)^\s*El .{0,300}escribi[óo]:\s*$").unwrap(),
            digit: Regex::new(r"\d").unwrap(),
            attribution_tail: Regex::new(
                r"(?i)^.{0,120}\S@\S+\s+(wrote|escreveu|escribi[óo]):\s*$",
            )
            .unwrap(),
            attribution_head: Regex::new(r"(?i)^\s*(On|Em|El) ").unwrap(),
            header_from_name: Regex::new(r"(?i)^\s*De:\s+\S").unwrap(),
            header_next: Regex::new(r"(?i)^\s*(Enviad[oa]( em| el)?:\s|(Data|Fecha):\s.*\d{4})")
                .unwrap(),
            signature_markers: vec![
                Regex::new(r"^\s*--\s*$").unwrap(),
                // English closing: closing word (case-insensitive, scoped) + optional extension +
                // punctuation + at most 3 capitalised name-words. The name class excludes lowercase
                // letters so `Regards, Łukasz` is a sign-off but `Thanks for the reply` is not.
                Regex::new(
                    r"^\s*(?i:best|kind|warmest|warm|many thanks|thanks|thank you|regards|cheers|sincerely)(?i:\s+(?:and|&)\s+regards|\s+(?:regards|wishes|again|in advance|a lot|so much|very much))?[\s,;:!.]*(?:[^\W\d_a-zß-öø-ÿ][\w'-]*[\s,.]*){0,3}$",
                )
                .unwrap(),
                Regex::new(r"(?i)^\s*sent from my (iphone|android|mobile|ipad)").unwrap(),
                // Portuguese/Spanish sign-offs: match only on their own, no trailing words allowed.
                Regex::new(
                    r"(?i)^\s*(atenciosamente|att|abraços?|abs|um abraço|cordialmente|grat[oa]|(muito )?obrigad[oa]s?( desde já| pela atenção)?|(com os melhores )?cumprimentos|saudações|(un )?saludos?( cordiales)?|atentamente|(muchas )?gracias( de antemano)?)[\s,!.]*$",
                )
                .unwrap(),
            ],
            device_footer: Regex::new(&device_footer).unwrap(),
            disclaimer: Regex::new(
                r"(?i)(\b(e-?mail|message|information|communication|transmission|contents?)\b[^.]{0,60}\bconfidential\b[^.]{0,60}\b(intended|solely|addressee|recipient|privileged|disclos|unauthori[sz]ed)|\bconfidential\b[^.]{0,60}\b(and (may|is) (also )?privileged)|if you (have )?received this (e-?mail|message) in error|\b(esta|este) (mensagem|e-?mail|mensaje|correo)\b[^.]{0,80}(confidencia|sigilos|privilegiad)|\b(uso exclusivo|exclusivamente|únicamente|unicamente)\b[^.]{0,30}(destinatári|destinatari|pessoa|persona|entidade|entidad)|\b(recebeu|recebido|receber) (esta|este) (mensagem|e-?mail)\b[^.]{0,20} por (engano|erro)|\b(ha recibido|recibió|recibe) (este|esta) (mensaje|correo)\b[^.]{0,20} por error|\bantes de imprimir\b[^.]{0,100}(meio ambiente|medio ambiente|natureza|planeta|realmente necess)|\b(meio|medio) ambiente\b[^.]{0,30}antes de imprimir)",
            )
            .unwrap(),
            paragraph_split: Regex::new(r"\n\s*\n").unwrap(),
            ws_collapse: Regex::new(r"[ \t]+").unwrap(),
        }
    })
}

/// True if any `_QUOTE_HEADERS` pattern matches the line (Python `re.match`, anchored at start).
fn is_quote_header(re: &Regexes, line: &str) -> bool {
    if re.quote_plain.iter().any(|p| p.is_match(line)) {
        return true;
    }
    // `Em (?=.*\d)…escreveu:` / `El (?=.*\d)…escribió:`: base pattern + a digit anywhere in line.
    let has_digit = re.digit.is_match(line);
    has_digit && (re.qh_em.is_match(line) || re.qh_el.is_match(line))
}

/// First letter is uppercase: a fresh sentence, not a wrapped line. Uncased scripts never start
/// a new piece. Mirrors Python `_starts_new_sentence`.
fn starts_new_sentence(line: &str) -> bool {
    for ch in line.chars() {
        if ch.is_alphabetic() {
            return ch.is_uppercase();
        }
    }
    false
}

/// Split a fused boilerplate-positive sentence at sentence-starting newlines. Mirrors
/// Python `_split_fused_lines`.
fn split_fused_lines(sentence: &str) -> Vec<String> {
    if !sentence.contains('\n') {
        return vec![sentence.to_string()];
    }
    let mut pieces: Vec<String> = Vec::new();
    let mut buf = String::new();
    for raw in sentence.split('\n') {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if !buf.is_empty() && starts_new_sentence(line) {
            pieces.push(std::mem::take(&mut buf));
            buf = line.to_string();
        } else if buf.is_empty() {
            buf = line.to_string();
        } else {
            buf.push(' ');
            buf.push_str(line);
        }
    }
    if !buf.is_empty() {
        pieces.push(buf);
    }
    pieces
}

/// Split `text` on any whitespace run immediately preceded by `.`, `!` or `?`, re-expressing
/// Python's `_SENTENCE = re.compile(r"(?<=[.!?])\s+")` look-behind. The whitespace is consumed.
fn split_sentence(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut result: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            if let Some(last) = current.chars().last() {
                if last == '.' || last == '!' || last == '?' {
                    // Split here; consume the whole whitespace run.
                    result.push(std::mem::take(&mut current));
                    while i < chars.len() && chars[i].is_whitespace() {
                        i += 1;
                    }
                    continue;
                }
            }
        }
        current.push(c);
        i += 1;
    }
    result.push(current);
    result
}

/// Drop boilerplate disclaimer text from one paragraph. Mirrors Python `_strip_disclaimer`.
fn strip_disclaimer(re: &Regexes, paragraph: &str) -> String {
    if !re.disclaimer.is_match(paragraph) {
        return paragraph.to_string(); // nothing to do: keep the original line structure
    }
    let parts: Vec<String> = split_sentence(paragraph)
        .into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    let mut pieces: Vec<String> = Vec::new();
    for p in &parts {
        if re.disclaimer.is_match(p) {
            pieces.extend(split_fused_lines(p));
        } else {
            pieces.push(p.clone());
        }
    }
    pieces
        .into_iter()
        .filter(|p| !re.disclaimer.is_match(p))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Remove quoted email history, signatures and disclaimers to keep input focused.
///
/// Python default `max_chars=3000`.
pub fn clean_email_body(body: &str) -> String {
    clean_email_body_with(body, 3000)
}

/// [`clean_email_body`] with an explicit `max_chars`.
pub fn clean_email_body_with(body: &str, max_chars: usize) -> String {
    let re = regexes();

    // Normalize newlines: \r\n, \r, and the literal two-char sequence \n all become real newlines.
    let text = body
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace("\\n", "\n");

    let src: Vec<&str> = text.split('\n').collect();
    let mut lines: Vec<String> = Vec::new();
    for (i, line) in src.iter().enumerate() {
        if is_quote_header(re, line) && !lines.is_empty() {
            break;
        }
        if !lines.is_empty()
            && re.header_from_name.is_match(line)
            && i + 1 < src.len()
            && re.header_next.is_match(src[i + 1])
        {
            break;
        }
        if re.attribution_tail.is_match(line) && !lines.is_empty() {
            // The `On/Em/El …` head this tail belongs to goes with it.
            // `_ATTRIBUTION_HEAD`: base pattern + a digit anywhere in the previous line.
            let prev = lines.last().unwrap();
            if re.attribution_head.is_match(prev) && re.digit.is_match(prev) {
                lines.pop();
            }
            break;
        }
        if line.trim_start().starts_with('>') {
            continue;
        }
        lines.push(line.trim_end().to_string());
    }

    // Signature / device-footer cut over range max(1, min(int(len*0.6), len-8)) .. len.
    let len = lines.len();
    let mut cut = len;
    let start_i: isize = std::cmp::max(
        1,
        std::cmp::min((len as f64 * 0.6) as isize, len as isize - 8),
    );
    let start = start_i as usize; // start >= 1, so non-negative
    for (i, line) in lines.iter().enumerate().skip(start) {
        let n = line.trim().chars().count();
        let sig = n <= 40 && re.signature_markers.iter().any(|p| p.is_match(line));
        let dev = n <= 60 && re.device_footer.is_match(line);
        if sig || dev {
            cut = i;
            break;
        }
    }
    lines.truncate(cut);

    // Split into paragraphs on blank lines, strip disclaimers, rejoin, collapse spaces, truncate.
    let joined = lines.join("\n");
    let paragraphs: Vec<String> = re
        .paragraph_split
        .split(&joined)
        .map(|p| strip_disclaimer(re, p))
        .collect();
    let combined = paragraphs
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let collapsed = re.ws_collapse.replace_all(&combined, " ");
    collapsed.chars().take(max_chars).collect()
}

/// Construct a clean state object for email classification.
///
/// Returns a JSON object `{"subject","body", optional "from", ...extra}`. `extra` entries whose
/// [`Value`] is null are skipped, and an empty `sender` is treated as absent (Python `if sender:`).
pub fn email_state(
    subject: &str,
    body: &str,
    sender: Option<&str>,
    clean: bool,
    extra: &IndexMap<String, Value>,
) -> Value {
    let mut map = serde_json::Map::new();
    map.insert(
        "subject".to_string(),
        Value::String(subject.trim().to_string()),
    );
    let body_val = if clean {
        clean_email_body(body)
    } else {
        body.to_string()
    };
    map.insert("body".to_string(), Value::String(body_val));
    if let Some(s) = sender {
        if !s.is_empty() {
            map.insert("from".to_string(), Value::String(s.to_string()));
        }
    }
    for (k, v) in extra {
        if !v.is_null() {
            map.insert(k.clone(), v.clone());
        }
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cuts_quoted_history() {
        let out = clean_email_body("Refund please\n\nOn Mon, Bob wrote:\nold text");
        assert!(out.contains("Refund please"));
        assert!(!out.contains("old text"));
    }

    #[test]
    fn cuts_device_footer() {
        let out = clean_email_body("Please refund my order\n\nSent from my iPhone");
        assert!(out.contains("Please refund my order"));
        assert!(!out.contains("iPhone"));
    }

    #[test]
    fn removes_english_signature() {
        let out = clean_email_body("Hi, please process my refund.\n\nBest regards,\nJohn Smith");
        assert!(out.contains("please process my refund"));
        assert!(!out.contains("Best regards"));
        assert!(!out.contains("John Smith"));
    }

    #[test]
    fn removes_portuguese_signoff() {
        let out = clean_email_body("Bom dia, preciso de ajuda.\n\nAtenciosamente,\nMaria");
        assert!(out.contains("preciso de ajuda"));
        assert!(!out.contains("Atenciosamente"));
        assert!(!out.contains("Maria"));
    }

    #[test]
    fn drops_confidentiality_footer() {
        let out = clean_email_body(
            "Please help.\n\nThis email is confidential and intended solely for the addressee.",
        );
        assert!(out.contains("Please help"));
        assert!(!out.to_lowercase().contains("confidential"));
    }

    #[test]
    fn keeps_body_mentioning_confidential() {
        // A bare mention is not a disclaimer: no noun anchor, no "privileged" tail.
        let out = clean_email_body("Is this confidential? I need a refund.");
        assert!(out.contains("confidential"));
        assert!(out.contains("I need a refund"));
    }

    #[test]
    fn email_state_builds_object() {
        let mut extra: IndexMap<String, Value> = IndexMap::new();
        extra.insert("priority".to_string(), json!("high"));
        extra.insert("skip".to_string(), Value::Null);
        let state = email_state(
            "  Order 123  ",
            "Refund please\n\nOn Mon, Bob wrote:\nold text",
            Some("a@b.com"),
            true,
            &extra,
        );
        assert_eq!(state["subject"], json!("Order 123"));
        assert_eq!(state["body"], json!("Refund please"));
        assert_eq!(state["from"], json!("a@b.com"));
        assert_eq!(state["priority"], json!("high"));
        assert!(state.get("skip").is_none());
    }

    #[test]
    fn email_state_skips_empty_sender_and_can_skip_clean() {
        let extra: IndexMap<String, Value> = IndexMap::new();
        let state = email_state("s", "raw\n\nOn X wrote:\nq", None, false, &extra);
        assert!(state.get("from").is_none());
        // clean=false keeps the body verbatim.
        assert_eq!(state["body"], json!("raw\n\nOn X wrote:\nq"));
    }
}
