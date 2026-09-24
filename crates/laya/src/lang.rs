//! Dependency-free language/script detection used to route between Laya checkpoints.
//!
//! Faithful port of `laya/lang.py`. Routing needs one decision: *is this English Latin text, or
//! is it something the English checkpoint cannot read?* Script detection is exact; the Latin-script
//! language guess is a stopword/diacritic heuristic and is explicitly best-effort.
//!
//! This is the source-of-truth port of the Python module (which carries `bn`/`az` stopword lists
//! and the `_non_latin_words` reclassification step that the older TypeScript reference dropped).

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use indexmap::IndexMap;
use regex::Regex;
use serde_json::Value;
use unicode_general_category::{get_general_category, GeneralCategory};

// Unicode blocks that the English (ModernBERT-large, 50k English BPE) checkpoint cannot read.
const SCRIPT_RANGES: &[(&str, &[(u32, u32)])] = &[
    ("greek", &[(0x0370, 0x03FF), (0x1F00, 0x1FFF)]),
    (
        "cyrillic",
        &[(0x0400, 0x052F), (0x2DE0, 0x2DFF), (0xA640, 0xA69F)],
    ),
    ("armenian", &[(0x0530, 0x058F)]),
    ("hebrew", &[(0x0590, 0x05FF)]),
    (
        "arabic",
        &[
            (0x0600, 0x06FF),
            (0x0750, 0x077F),
            (0x08A0, 0x08FF),
            (0xFB50, 0xFDFF),
            (0xFE70, 0xFEFF),
        ],
    ),
    ("devanagari", &[(0x0900, 0x097F), (0xA8E0, 0xA8FF)]),
    ("bengali", &[(0x0980, 0x09FF)]),
    ("gurmukhi", &[(0x0A00, 0x0A7F)]),
    ("gujarati", &[(0x0A80, 0x0AFF)]),
    ("oriya", &[(0x0B00, 0x0B7F)]),
    ("tamil", &[(0x0B80, 0x0BFF)]),
    ("telugu", &[(0x0C00, 0x0C7F)]),
    ("kannada", &[(0x0C80, 0x0CFF)]),
    ("malayalam", &[(0x0D00, 0x0D7F)]),
    ("sinhala", &[(0x0D80, 0x0DFF)]),
    ("thai", &[(0x0E00, 0x0E7F)]),
    ("lao", &[(0x0E80, 0x0EFF)]),
    ("tibetan", &[(0x0F00, 0x0FFF)]),
    ("myanmar", &[(0x1000, 0x109F)]),
    ("georgian", &[(0x10A0, 0x10FF)]),
    ("ethiopic", &[(0x1200, 0x137F)]),
    ("khmer", &[(0x1780, 0x17FF)]),
    (
        "hangul",
        &[(0x1100, 0x11FF), (0x3130, 0x318F), (0xAC00, 0xD7AF)],
    ),
    (
        "kana",
        &[(0x3040, 0x309F), (0x30A0, 0x30FF), (0x31F0, 0x31FF)],
    ),
    (
        "han",
        &[(0x3400, 0x4DBF), (0x4E00, 0x9FFF), (0xF900, 0xFAFF)],
    ),
];

/// A diacritic rate above this is taken as evidence the text is not English.
pub const NON_EN_DIACRITIC_RATE: f64 = 0.02;

/// Non-Latin text is not for the English checkpoint even when Latin letters are the plurality.
pub const NON_LATIN_FRACTION: f64 = 0.2;
pub const NON_LATIN_MIN_FRACTION: f64 = 0.1;
pub const NON_LATIN_MIN_LETTERS: i64 = 10;

/// Function words per language, in the exact insertion order of the Python `_STOP` dict.
fn stop() -> &'static IndexMap<&'static str, HashSet<&'static str>> {
    static S: OnceLock<IndexMap<&'static str, HashSet<&'static str>>> = OnceLock::new();
    S.get_or_init(|| {
        let mut m: IndexMap<&'static str, HashSet<&'static str>> = IndexMap::new();
        m.insert(
            "en",
            [
                "the", "and", "is", "are", "was", "were", "to", "of", "in", "for", "with", "that",
                "this", "it", "you", "have", "has", "not", "but", "on", "at", "be", "as", "from",
                "will", "can", "would", "there", "their", "what", "which", "please", "we", "i",
            ]
            .into_iter()
            .collect(),
        );
        m.insert(
            "fr",
            [
                "le", "la", "les", "des", "une", "est", "pour", "dans", "que", "qui", "avec",
                "sur", "pas", "plus", "nous", "vous", "être", "cette", "mais", "sont", "ont",
                "aux", "ce", "et", "du", "au", "ou", "je", "tu", "il", "elle", "ils", "elles",
                "mon", "ton", "ma", "ta", "sa", "mes", "tes", "ses", "ces", "deux", "trois",
                "très", "bien", "tout", "tous", "toute", "fait", "veux", "veut", "peux", "peut",
                "dois", "doit", "merci", "bonjour", "jour", "jours", "mois", "fois", "quand",
                "comment", "pourquoi", "alors", "donc",
            ]
            .into_iter()
            .collect(),
        );
        m.insert(
            "de",
            [
                "der", "die", "das", "und", "ist", "ein", "eine", "den", "dem", "nicht", "mit",
                "für", "auf", "von", "zu", "sich", "auch", "werden", "wurde", "haben", "sind",
                "oder", "aber", "ich", "wir", "mir", "mich", "dir", "dich", "uns", "mein", "meine",
                "meinen", "meinem", "meiner", "diese", "dieser", "diesen", "dieses", "einen",
                "einem", "einer", "wie", "wo", "wann", "welche", "im", "zum", "zur", "aus", "bei",
                "nach", "noch", "bitte", "heute", "jetzt", "kann", "kannst", "habe", "gibt",
                "wird",
                // shared with English on purpose: counted for English alone, they outvoted short German
                "in", "was",
            ]
            .into_iter()
            .collect(),
        );
        m.insert(
            "es",
            [
                "el", "los", "las", "que", "por", "con", "para", "una", "es", "se", "del", "como",
                "pero", "son", "está", "este", "esta", "todo", "más", "muy", "hay", "sus", "la",
                "un", "y", "al", "lo", "le", "les", "su", "mi", "tu", "nos", "ni", "dos", "tres",
                "fue", "fueron", "ser", "tiene", "tienen", "tengo", "puede", "pueden", "quiero",
                "necesito", "hemos", "han", "sobre", "entre", "cuando", "donde", "porque",
                "aunque", "también", "ya", "eso", "esto", "esa", "ese", "nada", "algo", "aquí",
                "hoy", "gracias",
            ]
            .into_iter()
            .collect(),
        );
        m.insert(
            "pt",
            [
                "os", "as", "que", "em", "um", "uma", "para", "com", "não", "é", "se", "do", "da",
                "dos", "das", "mas", "são", "está", "este", "esta", "muito", "pelo", "pela", "o",
                "e", "na", "nas", "nos", "ao", "aos", "por", "foi", "era", "ser", "sou", "tem",
                "tenho", "pode", "podem", "quero", "preciso", "eu", "meu", "minha", "seu", "sua",
                "isso", "isto", "aqui", "ali", "como", "quando", "onde", "porque", "mais", "já",
                "ainda", "agora", "hoje", "ontem", "dois", "três", "tudo", "nada", "obrigado",
                "olá", "você", "vocês", "voce", "voces", "vc", "vcs", "nao", "sao", "ja", "até",
                "tá", "pra", "gostaria", "obrigada", "também", "tambem", "estou", "estamos",
                "meus", "minhas", "nosso", "nossa", "consigo", "cadê", "boa", "tarde", "noite",
                "depois", "antes", "então", "entao", "ninguém", "ninguem", "alguém", "alguem",
                "nenhum", "nenhuma", "estava", "ficou", "fiz", "deu",
            ]
            .into_iter()
            .collect(),
        );
        m.insert(
            "it",
            [
                "il", "lo", "gli", "che", "di", "per", "con", "non", "è", "si", "del", "della",
                "sono", "questo", "questa", "anche", "come", "più", "sono", "nella", "alla", "la",
                "le", "un", "uno", "una", "e", "ed", "o", "da", "su", "tra", "fra", "mi", "ci",
                "ne", "ho", "hai", "ha", "abbiamo", "avete", "hanno", "era", "stato", "stata",
                "devo", "deve", "devono", "voglio", "vorrei", "mio", "mia", "tuo", "sua", "quando",
                "dove", "perche", "molto", "poco", "sempre", "mai", "già", "ancora", "adesso",
                "oggi", "ieri", "grazie", "ciao", "scusa", "nel", "nell", "negli", "sul", "sulla",
                "sulle", "dal", "dalla", "dallo", "dagli", "dei", "delle", "dello", "degli",
                "agli", "alle", "col",
            ]
            .into_iter()
            .collect(),
        );
        m.insert(
            "nl",
            [
                "het", "een", "van", "is", "op", "te", "dat", "niet", "met", "voor", "zijn", "aan",
                "door", "maar", "ook", "worden", "deze", "naar", "wordt",
            ]
            .into_iter()
            .collect(),
        );
        m.insert(
            "ro",
            [
                "și", "să", "este", "sunt", "care", "pentru", "din", "dar", "după", "până", "fără",
                "ale", "lui", "în", "fost", "acum", "vreau", "trebuie", "foarte", "acest",
                "această", "acesta", "aceasta", "mi", "ți", "vă", "nu",
            ]
            .into_iter()
            .collect(),
        );
        m.insert(
            "bn",
            [
                "ami",
                "amar",
                "amake",
                "amra",
                "amader",
                "apni",
                "apnar",
                "apnake",
                "apnara",
                "tumi",
                "tomar",
                "tomake",
                "tomra",
                "tader",
                "ota",
                "eita",
                "oita",
                "ekta",
                "ei",
                "oi",
                "ki",
                "keno",
                "kivabe",
                "kibhabe",
                "kothay",
                "kokhon",
                "kobe",
                "koto",
                "kintu",
                "jodi",
                "tahole",
                "ar",
                "theke",
                "jonno",
                "sathe",
                "shathe",
                "diye",
                "niye",
                "moddhe",
                "kore",
                "korte",
                "korchi",
                "korsi",
                "korbo",
                "korechi",
                "koreche",
                "korun",
                "koren",
                "korlam",
                "hobe",
                "hoyeche",
                "hoise",
                "hocche",
                "hoyni",
                "chai",
                "chaina",
                "lagbe",
                "parchi",
                "parbo",
                "parchina",
                "peyechi",
                "paini",
                "dite",
                "dilam",
                "diyechi",
                "nai",
                "khub",
                "onek",
                "ekhon",
                "akhon",
                "ekhono",
                "abar",
                "ekbar",
                "duibar",
                "ajke",
                "kalke",
                "taka",
                "bhalo",
                "valo",
                "kharap",
                "shomossa",
                "somossa",
                "dhonnobad",
                "bhai",
                "shob",
                "keu",
                "kichu",
                "bolte",
                "bolun",
                "parben",
                "asbe",
                "jabe",
                "pabo",
                "ferot",
                "dorkar",
                "hoye",
                "geche",
                "gese",
            ]
            .into_iter()
            .collect(),
        );
        m.insert(
            "az",
            [
                "və", "ve", "bir", "bu", "üçün", "ucun", "ilə", "ile", "olan", "olub", "olmasa",
                "var", "yox", "yoxdur", "mən", "sən", "biz", "siz", "onlar", "daha", "çox", "cox",
                "hər", "nə", "kimi", "görə", "sonra", "əgər", "eger", "deyil", "lakin", "amma",
                "ancaq", "artıq", "artiq", "də", "isə", "həm", "yalnız", "yalniz",
            ]
            .into_iter()
            .collect(),
        );
        m
    })
}

// Letters that ordinary English does not use.
const DIACRITICS: &str = concat!(
    "àâäãáåçéèêëíìîïñóòôöõøúùûüýÿßæœ", // Western European
    "ăâîșțşţ",                         // Romanian
    "ąćęłńśźż",                        // Polish
    "čďěňřšťůž",                       // Czech / Slovak
    "őű",                              // Hungarian
    "ğı",                              // Turkish (text is lowercased before matching)
    "āēģīķļņūž",                       // Baltic
    "đ",                               // Serbo-Croatian / Vietnamese
    "ə",                               // Azerbaijani
);

fn non_en_diacritics() -> &'static HashSet<char> {
    static S: OnceLock<HashSet<char>> = OnceLock::new();
    S.get_or_init(|| DIACRITICS.chars().collect())
}

/// Words that more than one list claims.
fn shared_words() -> &'static HashSet<&'static str> {
    static S: OnceLock<HashSet<&'static str>> = OnceLock::new();
    S.get_or_init(|| {
        let mut counts: HashMap<&'static str, usize> = HashMap::new();
        for words in stop().values() {
            for w in words {
                *counts.entry(*w).or_insert(0) += 1;
            }
        }
        counts
            .into_iter()
            .filter(|(_, n)| *n > 1)
            .map(|(w, _)| w)
            .collect()
    })
}

fn word_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"[^\W\d_]+").unwrap())
}

// A token whose dot or @ joins word characters is an identifier, not prose.
fn identifier_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"[\w-]*(?:[.@][\w-]+)+").unwrap())
}

/// A combining mark (Python `unicodedata.combining(ch) != 0`): Mn / Mc / Me.
fn is_combining(ch: char) -> bool {
    matches!(
        get_general_category(ch),
        GeneralCategory::NonspacingMark
            | GeneralCategory::SpacingMark
            | GeneralCategory::EnclosingMark
    )
}

/// `detect_script` / `script_profile` Latin test (`cp < 0x02B0`).
fn is_latin_detect(cp: u32) -> bool {
    cp < 0x02B0
        || (0x1E00..=0x1EFF).contains(&cp)
        || (0xFF21..=0xFF3A).contains(&cp)
        || (0xFF41..=0xFF5A).contains(&cp)
}

/// `_script_of` Latin test (`cp < 0x0250` — deliberately distinct from `detect_script`).
fn is_latin_script_of(cp: u32) -> bool {
    cp < 0x0250
        || (0x1E00..=0x1EFF).contains(&cp)
        || (0xFF21..=0xFF3A).contains(&cp)
        || (0xFF41..=0xFF5A).contains(&cp)
}

fn in_ranges(cp: u32, ranges: &[(u32, u32)]) -> bool {
    ranges.iter().any(|&(lo, hi)| lo <= cp && cp <= hi)
}

/// Round to 4 decimals, half-to-even (Python `round(x, 4)`).
fn round4(x: f64) -> f64 {
    round_half_even(x * 10000.0) / 10000.0
}

/// Round to the nearest integer, half-to-even (Python `round(x)`); valid for `x >= 0`.
fn round_half_even(x: f64) -> f64 {
    let floor = x.floor();
    let diff = x - floor;
    if diff < 0.5 {
        floor
    } else if diff > 0.5 {
        floor + 1.0
    } else {
        // exactly .5 -> round to even
        if (floor as i64) % 2 == 0 {
            floor
        } else {
            floor + 1.0
        }
    }
}

// --- text flattening -------------------------------------------------------

fn iter_text(v: &Value, depth: usize, out: &mut Vec<String>) {
    if depth > 6 {
        return;
    }
    match v {
        Value::Null => {}
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => {
            for x in a {
                iter_text(x, depth + 1, out);
            }
        }
        Value::Object(m) => {
            for x in m.values() {
                iter_text(x, depth + 1, out);
            }
        }
        _ => {}
    }
}

/// Flatten a state into the text used for detection (keys are ignored). `max_chars` default 4000.
pub fn state_text(state: &Value, max_chars: usize) -> String {
    let mut leaves: Vec<String> = Vec::new();
    iter_text(state, 0, &mut leaves);
    // Bound the join work before truncation: only `max_chars` are ever returned, so stop
    // materializing leaves once the budget is spent (parity with upstream `state_text`).
    let mut parts: Vec<String> = Vec::new();
    let mut budget = max_chars as isize;
    for leaf in leaves {
        if budget <= 0 {
            break;
        }
        let leaf_len = leaf.chars().count() as isize;
        if leaf_len > budget {
            parts.push(leaf.chars().take(budget as usize).collect());
            break;
        }
        parts.push(leaf);
        // Account for the joining space without materializing the full text first.
        budget -= leaf_len + 1;
    }
    parts.join(" ").chars().take(max_chars).collect()
}

// --- script detection ------------------------------------------------------

/// Dominant script of `text`: `"latin"`, `"han"`, ... or `"unknown"` if there are no letters.
pub fn detect_script(text: &str) -> String {
    let mut counts: IndexMap<&'static str, i64> = IndexMap::new();
    let mut latin: i64 = 0;
    for ch in text.chars() {
        if !ch.is_alphabetic() {
            continue;
        }
        let cp = ch as u32;
        if is_latin_detect(cp) {
            latin += 1;
            continue;
        }
        let mut found: Option<&'static str> = None;
        for (name, ranges) in SCRIPT_RANGES {
            if in_ranges(cp, ranges) {
                found = Some(name);
                break;
            }
        }
        let key = found.unwrap_or("other");
        *counts.entry(key).or_insert(0) += 1;
    }
    counts.insert("latin", latin);
    let total: i64 = counts.values().sum();
    if total == 0 {
        return "unknown".to_string();
    }
    // Python `max(..., key=value)` returns the first item on a tie (insertion order).
    let mut best: &str = "";
    let mut best_n: i64 = i64::MIN;
    for (k, v) in &counts {
        if *v > best_n {
            best_n = *v;
            best = k;
        }
    }
    best.to_string()
}

/// Fraction of alphabetic characters belonging to each detected script (nonzero entries only).
pub fn script_profile(text: &str) -> IndexMap<String, f64> {
    let mut counts: IndexMap<&'static str, i64> = IndexMap::new();
    counts.insert("latin", 0);
    for ch in text.chars() {
        if !ch.is_alphabetic() {
            continue;
        }
        let cp = ch as u32;
        if is_latin_detect(cp) {
            *counts.get_mut("latin").unwrap() += 1;
            continue;
        }
        let mut found: Option<&'static str> = None;
        for (name, ranges) in SCRIPT_RANGES {
            if in_ranges(cp, ranges) {
                found = Some(name);
                break;
            }
        }
        let key = found.unwrap_or("other");
        *counts.entry(key).or_insert(0) += 1;
    }
    let total: i64 = counts.values().sum();
    let mut out: IndexMap<String, f64> = IndexMap::new();
    if total == 0 {
        return out;
    }
    for (k, v) in &counts {
        if *v != 0 {
            out.insert((*k).to_string(), *v as f64 / total as f64);
        }
    }
    out
}

fn script_of(ch: char) -> Option<&'static str> {
    let cp = ch as u32;
    if is_latin_script_of(cp) {
        return None;
    }
    for (name, ranges) in SCRIPT_RANGES {
        if in_ranges(cp, ranges) {
            return Some(name);
        }
    }
    None
}

/// Non-Latin runs that read as words rather than as annotation inside English prose.
fn non_latin_words(text: &str) -> Vec<String> {
    let mut runs: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut script: Option<&'static str> = None;
    for ch in text.chars() {
        if is_combining(ch) {
            continue;
        }
        let s = script_of(ch);
        if s.is_some() && s == script {
            cur.push(ch);
            continue;
        }
        if !cur.is_empty() {
            runs.push(std::mem::take(&mut cur));
        }
        match s {
            Some(_) => {
                cur = ch.to_string();
                script = s;
            }
            None => {
                cur = String::new();
                script = None;
            }
        }
    }
    if !cur.is_empty() {
        runs.push(cur);
    }
    runs.into_iter()
        .filter(|w| {
            w.chars().count() >= 2 && !w.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
        })
        .collect()
}

// --- Latin-script language guess ------------------------------------------

/// Evidence behind the Latin-script language guess.
#[derive(Debug, Clone)]
pub struct LatinProfile {
    pub language: Option<String>,
    pub english_hits: usize,
    pub diacritic_rate: f64,
    pub looks_non_english: bool,
}

pub fn latin_profile(text: &str) -> LatinProfile {
    // Order matches Python: identifier-strip the ORIGINAL text, replace 'İ'->'i', lower, then findall.
    let stripped = identifier_re().replace_all(text, " ");
    let normalized = stripped.replace('\u{0130}', "i").to_lowercase();
    let words: Vec<&str> = word_re()
        .find_iter(&normalized)
        .map(|m| m.as_str())
        .collect();

    let lowered = text.to_lowercase();
    let diac_set = non_en_diacritics();
    let diac = lowered.chars().filter(|ch| diac_set.contains(ch)).count();
    let len = lowered.chars().count();
    let diac_rate = diac as f64 / (std::cmp::max(1, len) as f64);
    let non_english = diac_rate >= NON_EN_DIACRITIC_RATE;

    if words.len() < 4 {
        return LatinProfile {
            language: None,
            english_hits: 0,
            diacritic_rate: diac_rate,
            looks_non_english: non_english,
        };
    }

    let stop = stop();
    let shared = shared_words();

    // Per-language raw hit counts.
    let mut scores: IndexMap<&'static str, i64> = IndexMap::new();
    for (lg, sw) in stop {
        let s = words.iter().filter(|w| sw.contains(*w)).count() as i64;
        scores.insert(lg, s);
    }
    let en = scores.get("en").copied().unwrap_or(0);

    let word_set: HashSet<&str> = words.iter().copied().collect();

    // Only a language that matched at least one word no other list claims may be named.
    let mut best_lg: Option<&'static str> = None;
    let mut best: i64 = 0;
    for (lg, sw) in stop {
        if *lg == "en" {
            continue;
        }
        let has_evidence = word_set
            .iter()
            .any(|w| sw.contains(*w) && !shared.contains(*w));
        if !has_evidence {
            continue;
        }
        let s = *scores.get(lg).unwrap();
        if s > best {
            best = s;
            best_lg = Some(lg);
        }
    }

    let mut language: Option<String> = None;
    if let Some(bl) = best_lg {
        // A non-English language needs a clear margin over English function words, or (with
        // non-English letters present) at least a two-hit tie. Mirrors the elif ladder in
        // laya/lang.py, with the two "name best_lg" branches combined.
        if best >= std::cmp::max(2, en + 2) || (non_english && best >= std::cmp::max(2, en)) {
            language = Some(bl.to_string());
        } else if en > 0 && !non_english {
            language = Some("en".to_string());
        }
    } else if en > 0 && !non_english {
        language = Some("en".to_string());
    }

    LatinProfile {
        language,
        english_hits: en as usize,
        diacritic_rate: diac_rate,
        looks_non_english: non_english,
    }
}

/// Best-effort language code for Latin-script text, or `None` when undecided.
pub fn guess_latin_language(text: &str) -> Option<String> {
    latin_profile(text).language
}

// --- top-level analysis ----------------------------------------------------

/// Result of [`analyse`]. Field names/semantics mirror the Python dict keys.
#[derive(Debug, Clone)]
pub struct Analysis {
    pub script: String,
    pub script_profile: IndexMap<String, f64>,
    pub language: Option<String>,
    pub is_english: bool,
    pub language_undecided: bool,
    pub diacritic_rate: f64,
    pub non_latin_fraction: f64,
}

pub fn analyse(state: &Value) -> Analysis {
    let text = state_text(state, 4000);
    let prof = script_profile(&text);
    let mut script = detect_script(&text);

    let non_latin = if !prof.is_empty() {
        round4(1.0 - prof.get("latin").copied().unwrap_or(0.0))
    } else {
        0.0
    };
    let sum_alpha = text.chars().filter(|c| c.is_alphabetic()).count() as f64;
    let n_non_latin = round_half_even(non_latin * sum_alpha) as i64;

    if script == "latin"
        && !non_latin_words(&text).is_empty()
        && (non_latin >= NON_LATIN_FRACTION
            || (non_latin >= NON_LATIN_MIN_FRACTION && n_non_latin >= NON_LATIN_MIN_LETTERS))
    {
        // reclassify to the dominant non-latin script (first max on a tie, insertion order)
        let mut best: Option<&str> = None;
        let mut best_v = f64::NEG_INFINITY;
        for (k, v) in &prof {
            if k == "latin" {
                continue;
            }
            if *v > best_v {
                best_v = *v;
                best = Some(k);
            }
        }
        if let Some(b) = best {
            script = b.to_string();
        }
    }

    if script == "unknown" {
        return Analysis {
            script: "unknown".to_string(),
            script_profile: prof,
            language: None,
            is_english: true,
            language_undecided: true,
            diacritic_rate: 0.0,
            non_latin_fraction: 0.0,
        };
    }
    if script != "latin" {
        return Analysis {
            script,
            script_profile: prof,
            language: None,
            is_english: false,
            language_undecided: true,
            diacritic_rate: 0.0,
            non_latin_fraction: non_latin,
        };
    }

    let prof_lat = latin_profile(&text);
    let lang = prof_lat.language.clone();
    let undecided = lang.is_none();
    let english = lang.as_deref() == Some("en") || (undecided && !prof_lat.looks_non_english);
    Analysis {
        script: "latin".to_string(),
        script_profile: prof,
        language: lang,
        is_english: english,
        language_undecided: undecided,
        diacritic_rate: round4(prof_lat.diacritic_rate),
        non_latin_fraction: non_latin,
    }
}

/// True when the English checkpoint can be expected to read this state.
pub fn is_english(state: &Value) -> bool {
    analyse(state).is_english
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn s(t: &str) -> Value {
        Value::String(t.to_string())
    }

    #[test]
    fn detects_devanagari_as_non_latin() {
        assert_eq!(detect_script("मुझसे दो बार शुल्क लिया गया"), "devanagari");
    }

    #[test]
    fn routes_english_latin_to_english() {
        let a = analyse(&s("Please refund the duplicate charge"));
        assert_eq!(a.script, "latin");
        assert!(a.is_english);
        assert_eq!(a.language.as_deref(), Some("en"));
    }

    #[test]
    fn routes_german_latin_to_non_english() {
        assert!(!is_english(&s("Der Kunde wurde zweimal belastet")));
    }

    #[test]
    fn detects_french_multilingual_signals() {
        let a = analyse(&s("Je voudrais un remboursement pour la commande"));
        assert!(!a.is_english);
        assert_eq!(a.language.as_deref(), Some("fr"));
    }

    #[test]
    fn unknown_no_letters_is_english_and_undecided() {
        let a = analyse(&s("123 !!!"));
        assert_eq!(a.script, "unknown");
        assert!(a.is_english);
        assert!(a.language_undecided);
    }

    #[test]
    fn cjk_han_is_not_english() {
        let a = analyse(&s("请退还重复的费用"));
        assert_eq!(a.script, "han");
        assert!(!a.is_english);
    }

    #[test]
    fn hindi_devanagari_is_not_english() {
        let a = analyse(&s("मुझसे दो बार शुल्क लिया गया"));
        assert_eq!(a.script, "devanagari");
        assert!(!a.is_english);
    }

    #[test]
    fn romanized_bangla_detected() {
        let a = analyse(&s("ami ekta refund chai amar order er jonno"));
        assert_eq!(a.script, "latin");
        assert_eq!(a.language.as_deref(), Some("bn"));
        assert!(!a.is_english);
    }

    #[test]
    fn url_only_not_misdetected_as_foreign() {
        // Without identifier stripping, four "com" tokens would score Portuguese/Italian.
        let a = analyse(&s("acme.com foo.com bar.com test.com"));
        assert_eq!(a.language, None);
        assert!(a.is_english);
    }

    #[test]
    fn script_profile_latin_first_and_fractions() {
        let p = script_profile("abc देव");
        // latin inserted first; both scripts present.
        let keys: Vec<&String> = p.keys().collect();
        assert_eq!(keys[0], "latin");
        assert!(p.contains_key("devanagari"));
        let sum: f64 = p.values().sum();
        assert!((sum - 1.0).abs() < 1e-9);
    }

    #[test]
    fn nested_state_is_flattened() {
        let state = json!({"a": "Please refund", "b": ["the", "duplicate", "charge"]});
        assert!(is_english(&state));
    }

    #[test]
    fn round4_is_half_to_even() {
        assert_eq!(round_half_even(2.5), 2.0);
        assert_eq!(round_half_even(3.5), 4.0);
    }
}
