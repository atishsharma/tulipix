// The Kitchen section's store, and the parsing it rests on.
//
// `kitchen.db` holds recipes (as lines of ingredients and steps), the pantry
// (what you have, by ingredient name), the week's plan and the shopping list.
// The parts worth testing live here as plain functions: reading a quantity
// and a unit off an ingredient line, scaling and printing it again, finding
// timers in a step, and pulling a schema.org Recipe out of a web page.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde_json::Value;
use sqlx::SqlitePool;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS recipes (
    id          INTEGER PRIMARY KEY,
    title       TEXT    NOT NULL,
    source_url  TEXT    NOT NULL DEFAULT '',
    image       TEXT    NOT NULL DEFAULT '',   -- a local file, or ''
    minutes     INTEGER NOT NULL DEFAULT 0,
    servings    INTEGER NOT NULL DEFAULT 4,
    category    TEXT    NOT NULL DEFAULT '',
    favourite   INTEGER NOT NULL DEFAULT 0,
    cooked      INTEGER NOT NULL DEFAULT 0,
    last_cooked INTEGER NOT NULL DEFAULT 0,
    created     INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS ingredients (
    id        INTEGER PRIMARY KEY,
    recipe_id INTEGER NOT NULL REFERENCES recipes(id) ON DELETE CASCADE,
    pos       INTEGER NOT NULL,
    line      TEXT    NOT NULL,              -- as written
    qty       REAL,                          -- NULL: "salt", "a handful"
    unit      TEXT    NOT NULL DEFAULT '',
    name      TEXT    NOT NULL,
    note      TEXT    NOT NULL DEFAULT '',
    key       TEXT    NOT NULL               -- what the pantry and the list match on
);
CREATE INDEX IF NOT EXISTS ingredients_recipe_idx ON ingredients(recipe_id, pos);

CREATE TABLE IF NOT EXISTS steps (
    recipe_id INTEGER NOT NULL REFERENCES recipes(id) ON DELETE CASCADE,
    pos       INTEGER NOT NULL,
    text      TEXT    NOT NULL,
    PRIMARY KEY (recipe_id, pos)
);

-- What is in the kitchen, by ingredient key.
CREATE TABLE IF NOT EXISTS pantry (
    key TEXT PRIMARY KEY
);

CREATE TABLE IF NOT EXISTS plan (
    day       TEXT    NOT NULL,              -- ISO date
    meal      TEXT    NOT NULL,              -- lunch | dinner
    recipe_id INTEGER REFERENCES recipes(id) ON DELETE SET NULL,
    label     TEXT    NOT NULL DEFAULT '',   -- "Out", "Leftover tacos"
    PRIMARY KEY (day, meal)
);

CREATE TABLE IF NOT EXISTS shop (
    id      INTEGER PRIMARY KEY,
    key     TEXT    NOT NULL,
    name    TEXT    NOT NULL,
    qty     REAL,
    unit    TEXT    NOT NULL DEFAULT '',
    aisle   TEXT    NOT NULL,
    done    INTEGER NOT NULL DEFAULT 0,
    -- plan: made from the week, remade with it · recipe · typed
    origin  TEXT    NOT NULL DEFAULT 'typed',
    created INTEGER NOT NULL
);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

// ── ingredients ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Ing {
    pub qty: Option<f64>,
    pub unit: String,
    pub name: String,
    pub note: String,
}

/// Every spelling of a unit, and the one it is written as.
const UNITS: &[(&str, &[&str])] = &[
    ("g", &["g", "gr", "gram", "grams", "gramme", "grammes"]),
    ("kg", &["kg", "kgs", "kilo", "kilos", "kilogram", "kilograms"]),
    ("ml", &["ml", "millilitre", "millilitres", "milliliter", "milliliters"]),
    ("l", &["l", "litre", "litres", "liter", "liters"]),
    ("tsp", &["tsp", "tsps", "teaspoon", "teaspoons", "t"]),
    ("tbsp", &["tbsp", "tbsps", "tbs", "tablespoon", "tablespoons", "tbl"]),
    ("cup", &["cup", "cups"]),
    ("oz", &["oz", "ounce", "ounces"]),
    ("lb", &["lb", "lbs", "pound", "pounds"]),
    ("clove", &["clove", "cloves"]),
    ("bunch", &["bunch", "bunches"]),
    ("tin", &["tin", "tins"]),
    ("can", &["can", "cans"]),
    ("jar", &["jar", "jars"]),
    ("pinch", &["pinch", "pinches"]),
    ("handful", &["handful", "handfuls"]),
    ("slice", &["slice", "slices"]),
    ("sprig", &["sprig", "sprigs"]),
    ("stick", &["stick", "sticks"]),
    ("pack", &["pack", "packs", "packet", "packets"]),
    ("piece", &["piece", "pieces"]),
];

fn unit_of(word: &str) -> Option<&'static str> {
    let w = word.trim_end_matches('.').to_lowercase();
    // "T" is a tablespoon and "t" a teaspoon in American books; lowercase
    // loses that, so a bare letter only counts as teaspoon when it was "t".
    if word == "T" {
        return Some("tbsp");
    }
    UNITS.iter().find(|(_, all)| all.contains(&w.as_str())).map(|(u, _)| *u)
}

fn fraction(c: char) -> Option<f64> {
    Some(match c {
        '½' => 0.5,
        '⅓' => 1.0 / 3.0,
        '⅔' => 2.0 / 3.0,
        '¼' => 0.25,
        '¾' => 0.75,
        '⅛' => 0.125,
        '⅜' => 0.375,
        '⅝' => 0.625,
        '⅞' => 0.875,
        '⅕' => 0.2,
        _ => return None,
    })
}

/// "1", "1.5", "1,5", "1/2", "½", "1½". None when it is not a number.
fn number(tok: &str) -> Option<f64> {
    let mut whole = String::new();
    let mut frac = 0.0;
    for c in tok.chars() {
        if let Some(f) = fraction(c) {
            frac += f;
        } else {
            whole.push(if c == ',' { '.' } else { c });
        }
    }
    if whole.is_empty() {
        return (frac > 0.0).then_some(frac);
    }
    if let Some((a, b)) = whole.split_once('/') {
        let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
        return (b != 0.0).then(|| a / b + frac);
    }
    whole.parse::<f64>().ok().filter(|v| v.is_finite()).map(|v| v + frac)
}

/// "800g" → (800, "g"); "2x" is not a unit and stays whole.
fn glued(tok: &str) -> Option<(f64, &'static str)> {
    let at = tok.find(|c: char| c.is_alphabetic())?;
    if at == 0 {
        return None;
    }
    Some((number(&tok[..at])?, unit_of(&tok[at..])?))
}

/// An ingredient line as a quantity, a unit, a name and a note: "2 tbsp olive
/// oil", "800g chopped tomatoes", "1½ cups flour, sifted", "3 garlic cloves
/// (crushed)", "salt".
pub fn parse_ingredient(line: &str) -> Ing {
    let line = line.trim().trim_start_matches(['-', '*', '•', '·']).trim();
    // The note: what follows the first comma, or anything in brackets.
    let (mut head, mut note) = match line.split_once(',') {
        Some((a, b)) => (a.to_string(), b.trim().to_string()),
        None => (line.to_string(), String::new()),
    };
    if let (Some(a), Some(b)) = (head.find('('), head.rfind(')')) {
        if a < b {
            let inside = head[a + 1..b].trim().to_string();
            head = format!("{}{}", &head[..a], &head[b + 1..]);
            note = if note.is_empty() { inside } else { format!("{inside}, {note}") };
        }
    }
    let toks: Vec<&str> = head.split_whitespace().collect();
    let mut i = 0;
    let mut qty: Option<f64> = None;
    let mut unit = "";
    if let Some((q, u)) = toks.first().and_then(|t| glued(t)) {
        qty = Some(q);
        unit = u;
        i = 1;
    } else if let Some(q) = toks.first().and_then(|t| number(t.split(['-', '–']).next().unwrap_or(t))) {
        qty = Some(q);
        i = 1;
        // "1 1/2", or "2 - 3": the second number is part of the first.
        if let Some(f) = toks.get(1).filter(|t| t.contains('/')).and_then(|t| number(t)) {
            qty = Some(q + f);
            i = 2;
        } else if toks.get(1).is_some_and(|t| *t == "-" || *t == "–" || *t == "to") && toks.get(2).and_then(|t| number(t)).is_some() {
            i = 3;
        }
    }
    if qty.is_some() {
        if let Some(u) = toks.get(i).and_then(|t| unit_of(t)) {
            unit = u;
            i += 1;
        }
    }
    let mut rest: Vec<&str> = toks[i.min(toks.len())..].to_vec();
    if rest.first().is_some_and(|w| w.eq_ignore_ascii_case("of")) {
        rest.remove(0);
    }
    Ing { qty, unit: unit.to_string(), name: rest.join(" "), note }
}

/// Words that say how an ingredient is cut or chosen, not what it is — "2
/// finely chopped onions" is still onions on a shopping list.
const PREP: &[&str] = &[
    "fresh", "large", "small", "medium", "big", "chopped", "diced", "sliced", "minced", "finely",
    "roughly", "grated", "crushed", "peeled", "tinned", "canned", "ripe", "whole", "dried", "free-range",
    "organic", "good", "quality", "extra", "virgin", "plain", "a", "an", "some", "few",
];

/// The name the pantry and the list match on: lowercase, prep words and unit
/// words out, the last word made singular. "Garlic cloves" and "3 cloves
/// garlic" both come out as "garlic".
pub fn key(name: &str) -> String {
    let words: Vec<String> = name
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '-')
        .filter(|w| !w.is_empty() && !PREP.contains(w) && unit_of(w).is_none())
        .map(str::to_string)
        .collect();
    let mut words = words;
    if let Some(last) = words.last_mut() {
        *last = singular(last);
    }
    words.join(" ")
}

fn singular(w: &str) -> String {
    if w.len() > 4 && w.ends_with("ies") {
        format!("{}y", &w[..w.len() - 3])
    } else if w.len() > 4 && (w.ends_with("oes") || w.ends_with("ches") || w.ends_with("shes")) {
        w[..w.len() - 2].to_string()
    } else if w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") && !w.ends_with("us") {
        w[..w.len() - 1].to_string()
    } else {
        w.to_string()
    }
}

/// Always in the kitchen, never on a list, never "not in stock".
pub fn staple(key: &str) -> bool {
    matches!(key, "salt" | "pepper" | "black pepper" | "water" | "salt and pepper" | "ice" | "sea salt")
}

/// A quantity as a cook writes it: grams and millilitres rounded, spoons and
/// cups as the nearest friendly fraction, everything else to one decimal.
pub fn fmt_qty(q: f64, unit: &str) -> String {
    if q <= 0.0 {
        return String::new();
    }
    if matches!(unit, "g" | "ml") {
        let r = if q >= 100.0 { (q / 5.0).round() * 5.0 } else { q.round() };
        return format!("{r:.0}");
    }
    let whole = q.trunc();
    let part = q - whole;
    let fracs = [(0.0, ""), (0.25, "¼"), (1.0 / 3.0, "⅓"), (0.5, "½"), (2.0 / 3.0, "⅔"), (0.75, "¾"), (1.0, "")];
    let (f, glyph) = fracs
        .iter()
        .min_by(|a, b| (a.0 - part).abs().total_cmp(&(b.0 - part).abs()))
        .copied()
        .unwrap_or((0.0, ""));
    if (f - part).abs() > 0.05 {
        let s = format!("{q:.1}");
        return s.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    let whole = whole + if f == 1.0 { 1.0 } else { 0.0 };
    match (whole as i64, glyph) {
        (0, "") => "0".into(),
        (0, g) => g.into(),
        (w, g) => format!("{w}{g}"),
    }
}

/// "2 tbsp", "800 g", "6", or "" for no quantity.
pub fn amount(qty: Option<f64>, unit: &str, factor: f64) -> String {
    match qty {
        None => String::new(),
        Some(q) => {
            let n = fmt_qty(q * factor, unit);
            if unit.is_empty() { n } else { format!("{n} {unit}") }
        }
    }
}

// ── steps ───────────────────────────────────────────────────────────────────

/// Verbs a timer is named after, first one in the step wins.
const VERBS: &[&str] = &[
    "simmer", "boil", "bake", "roast", "fry", "cook", "soften", "rest", "chill", "marinate", "knead",
    "prove", "rise", "steam", "grill", "toast", "reduce", "stir", "brown", "blanch", "poach", "leave",
];

/// Timers a step asks for: "simmer for 10 minutes", "bake 1 hour 20 min",
/// "8-10 mins". A range takes its lower end — check early, not late.
pub fn timers(step: &str) -> Vec<(String, i64)> {
    let lower = step.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| c.is_whitespace() || c == ',' || c == ';' || c == '(' || c == ')').filter(|w| !w.is_empty()).collect();
    let verb = words
        .iter()
        .map(|w| w.trim_matches(|c: char| !c.is_alphabetic()))
        .find(|w| VERBS.contains(w))
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
        })
        .unwrap_or_else(|| "Timer".into());
    let mut out: Vec<(String, i64)> = Vec::new();
    let mut secs = 0i64;
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        // "10", "8-10", "10min", "1½"
        let (num_part, glued_unit) = match w.find(|c: char| c.is_alphabetic()) {
            Some(at) if at > 0 => (&w[..at], Some(&w[at..])),
            _ => (w, None),
        };
        let first = num_part.split(['-', '–']).next().unwrap_or(num_part);
        let Some(n) = number(first) else {
            i += 1;
            continue;
        };
        let mut unit_word = glued_unit;
        let mut step_over = 1;
        if unit_word.is_none() {
            // "8 to 10 minutes" / "8 - 10 minutes"
            let mut j = i + 1;
            if words.get(j).is_some_and(|t| *t == "to" || *t == "-" || *t == "–") {
                j += 2;
            }
            unit_word = words.get(j).copied();
            step_over = j - i + 1;
        }
        let per = match unit_word.map(|u| u.trim_end_matches('.')) {
            Some("h" | "hr" | "hrs" | "hour" | "hours") => 3600.0,
            Some("m" | "min" | "mins" | "minute" | "minutes") => 60.0,
            Some("s" | "sec" | "secs" | "second" | "seconds") => 1.0,
            _ => {
                i += 1;
                continue;
            }
        };
        secs += (n * per).round() as i64;
        i += step_over;
        // "1 hour 20 minutes" is one timer; anything else between ends it.
        let joined = words.get(i).and_then(|t| number(t)).is_some()
            || words.get(i).is_some_and(|t| *t == "and") && words.get(i + 1).and_then(|t| number(t)).is_some();
        if words.get(i).is_some_and(|t| *t == "and") && joined {
            i += 1;
        }
        if !joined && secs > 0 {
            out.push((verb.clone(), secs));
            secs = 0;
        }
    }
    if secs > 0 {
        out.push((verb, secs));
    }
    out
}

/// The ingredients a step mentions, by the last word of their name — "tomatoes"
/// finds "800 g chopped tomatoes" — in the recipe's order.
pub fn uses(step: &str, names: &[String]) -> Vec<usize> {
    let lower = step.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    names
        .iter()
        .enumerate()
        .filter(|(_, n)| {
            let k = key(n);
            let Some(last) = k.split_whitespace().last() else { return false };
            // By the start of a word: "tomato" finds "tomatoes", "oil" does not find "boil".
            last.len() > 2 && words.iter().any(|w| w.starts_with(last))
        })
        .map(|(i, _)| i)
        .collect()
}

// ── aisles ──────────────────────────────────────────────────────────────────

pub const AISLES: [&str; 6] = ["Veg and fruit", "Fridge", "Bakery", "Cupboard", "Frozen", "Other"];

/// Which part of the shop an ingredient is in, by what its key contains.
///
/// ponytail: a word table. Learn from where the user moves an item if the
/// guesses start to grate.
pub fn aisle(key: &str) -> &'static str {
    const TABLE: &[(&str, &[&str])] = &[
        ("Frozen", &["frozen", "ice cream"]),
        ("Fridge", &[
            "milk", "butter", "cheese", "feta", "yoghurt", "yogurt", "cream", "egg", "chicken", "beef", "pork",
            "lamb", "mince", "bacon", "ham", "sausage", "salmon", "fish", "prawn", "tofu", "halloumi", "mozzarella",
            "parmesan", "creme fraiche", "fillet",
        ]),
        ("Bakery", &["bread", "loaf", "bun", "roll", "tortilla", "wrap", "pitta", "naan", "bagel", "croissant", "pastry"]),
        ("Veg and fruit", &[
            "onion", "garlic", "pepper", "tomato", "potato", "carrot", "leek", "celery", "courgette", "aubergine",
            "mushroom", "spinach", "lettuce", "cabbage", "kale", "broccoli", "cauliflower", "cucumber", "chilli",
            "ginger", "lemon", "lime", "orange", "apple", "banana", "berry", "avocado", "herb", "coriander",
            "parsley", "basil", "mint", "thyme", "rosemary", "spring onion", "shallot", "squash", "sweet potato",
            "bean sprout", "pak choi", "rocket", "fennel", "beetroot", "grape", "pear", "mango", "pea",
        ]),
        ("Cupboard", &[
            "oil", "vinegar", "flour", "sugar", "rice", "pasta", "noodle", "spaghetti", "lentil", "chickpea", "bean",
            "stock", "tin", "sauce", "paste", "spice", "cumin", "paprika", "turmeric", "cinnamon", "oregano",
            "curry", "honey", "syrup", "salt", "pepper", "yeast", "baking", "cocoa", "chocolate", "oat", "nut",
            "seed", "coconut milk", "chopped tomato", "passata", "soy", "miso", "mustard", "mayonnaise", "arborio",
            "couscous", "quinoa", "vanilla", "jam", "stock cube", "chicken stock", "vegetable stock", "beef stock",
        ]),
    ];
    // Longest match wins, so "coconut milk" is the cupboard's and not the
    // fridge's "milk", and "chopped tomato" a tin rather than a vegetable.
    let mut best: Option<(&str, usize)> = None;
    for (aisle, words) in TABLE {
        for w in *words {
            if key.contains(w) && best.is_none_or(|(_, n)| w.len() > n) {
                best = Some((aisle, w.len()));
            }
        }
    }
    best.map(|b| b.0).unwrap_or("Other")
}

/// Shopping lines summed by key and unit, keeping the first spelling of the
/// name. A line with no quantity makes its row quantity-less: "salt" plus
/// "1 tsp salt" is not "1 tsp salt".
pub fn merge(lines: &[(String, String, Option<f64>, String)]) -> Vec<(String, String, Option<f64>, String)> {
    let mut by: BTreeMap<(String, String), (String, Option<f64>, bool)> = BTreeMap::new();
    let mut order: Vec<(String, String)> = Vec::new();
    for (k, name, qty, unit) in lines {
        let id = (k.clone(), unit.clone());
        match by.get_mut(&id) {
            Some(row) => {
                row.1 = match (row.1, qty) {
                    (Some(a), Some(b)) => Some(a + b),
                    _ => None,
                };
                row.2 |= qty.is_none();
            }
            None => {
                by.insert(id.clone(), (name.clone(), *qty, qty.is_none()));
                order.push(id);
            }
        }
    }
    order
        .into_iter()
        .filter_map(|id| {
            let (name, qty, blank) = by.get(&id)?.clone();
            Some((id.0, name, if blank { None } else { qty }, id.1))
        })
        .collect()
}

// ── the web ─────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Draft {
    pub title: String,
    pub image: String,
    pub minutes: i64,
    pub servings: i64,
    pub category: String,
    pub ingredients: Vec<String>,
    pub steps: Vec<String>,
}

/// Every `<script type="application/ld+json">` body on a page.
fn ld_blocks(html: &str) -> Vec<&str> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find("<script") {
        let start = from + at;
        let Some(close) = lower[start..].find('>') else { break };
        let tag = &lower[start..start + close];
        let body = start + close + 1;
        let Some(end) = lower[body..].find("</script") else { break };
        if tag.contains("ld+json") {
            out.push(&html[body..body + end]);
        }
        from = body + end;
    }
    out
}

fn is_recipe(v: &Value) -> bool {
    match v.get("@type") {
        Some(Value::String(s)) => s.eq_ignore_ascii_case("recipe"),
        Some(Value::Array(a)) => a.iter().any(|t| t.as_str().is_some_and(|s| s.eq_ignore_ascii_case("recipe"))),
        _ => false,
    }
}

/// The first Recipe anywhere in a JSON-LD tree: at the top, in an array, or
/// in an `@graph` — every site puts it somewhere else.
fn find_recipe(v: &Value) -> Option<&Value> {
    if is_recipe(v) {
        return Some(v);
    }
    match v {
        Value::Array(a) => a.iter().find_map(find_recipe),
        Value::Object(o) => o.get("@graph").and_then(find_recipe).or_else(|| o.get("mainEntity").and_then(find_recipe)),
        _ => None,
    }
}

/// Text as a page means it: entities decoded, tags gone, space collapsed.
fn clean(s: &str) -> String {
    let t = if s.contains('<') || s.contains('&') { tulipix_books::epub::html_to_text(s) } else { s.to_string() };
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn text_of(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(clean(s)),
        Value::Number(n) => Some(n.to_string()),
        Value::Array(a) => a.iter().find_map(text_of),
        Value::Object(o) => o.get("url").or_else(|| o.get("text")).or_else(|| o.get("name")).and_then(text_of),
        _ => None,
    }
}

fn texts(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a.iter().filter_map(text_of).filter(|s| !s.is_empty()).collect(),
        Some(Value::String(s)) => s.split(',').map(clean).filter(|s| !s.is_empty()).collect(),
        _ => Vec::new(),
    }
}

/// "PT1H30M", "P0DT0H35M" → minutes.
pub fn iso_minutes(s: &str) -> i64 {
    let s = s.trim().to_ascii_uppercase();
    let Some(rest) = s.strip_prefix('P') else { return 0 };
    let (mut total, mut num, mut in_time) = (0.0, String::new(), false);
    for c in rest.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' | '.' => num.push(c),
            _ => {
                let n: f64 = num.parse().unwrap_or(0.0);
                num.clear();
                total += match (c, in_time) {
                    ('D', _) => n * 1440.0,
                    ('H', true) => n * 60.0,
                    ('M', true) => n,
                    ('S', true) => n / 60.0,
                    _ => 0.0,
                };
            }
        }
    }
    total.round() as i64
}

/// Steps from `recipeInstructions`, which is a string, a list of strings, a
/// list of HowToSteps, or HowToSections holding HowToSteps.
fn steps_of(v: Option<&Value>) -> Vec<String> {
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.extend(
                clean_lines(s).into_iter().filter(|l| !l.is_empty()),
            ),
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            Value::Object(o) => {
                if let Some(items) = o.get("itemListElement") {
                    walk(items, out);
                } else if let Some(t) = o.get("text").or_else(|| o.get("name")).and_then(Value::as_str) {
                    let t = clean(t);
                    if !t.is_empty() {
                        out.push(t);
                    }
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    if let Some(v) = v {
        walk(v, &mut out);
    }
    out
}

/// A block of instructions as its lines — one step per line or paragraph.
fn clean_lines(s: &str) -> Vec<String> {
    let t = if s.contains('<') { tulipix_books::epub::html_to_text(s) } else { s.to_string() };
    t.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|l| !l.is_empty()).collect()
}

/// The category a card shows, from whatever the page says about diet and kind.
fn category_of(r: &Value) -> String {
    let mut words: Vec<String> = texts(r.get("suitableForDiet"));
    words.extend(texts(r.get("recipeCategory")));
    words.extend(texts(r.get("keywords")));
    let all = words.join(" ").to_lowercase();
    for (needle, name) in [
        ("vegan", "Vegan"),
        ("vegetarian", "Vegetarian"),
        ("bak", "Baking"),
        ("bread", "Baking"),
        ("cake", "Baking"),
        ("dessert", "Dessert"),
        ("fish", "Fish"),
        ("seafood", "Fish"),
    ] {
        if all.contains(needle) {
            return name.into();
        }
    }
    texts(r.get("recipeCategory")).into_iter().next().unwrap_or_default()
}

/// The first number in a yield: 4, "4 servings", "Serves 4–6", ["4", "4 bowls"].
fn servings_of(v: Option<&Value>) -> i64 {
    let s = match v {
        Some(Value::Number(n)) => return n.as_f64().map(|f| f.round() as i64).unwrap_or(4).clamp(1, 100),
        Some(v) => text_of(v).unwrap_or_default(),
        None => String::new(),
    };
    s.split(|c: char| !c.is_ascii_digit()).find(|w| !w.is_empty()).and_then(|w| w.parse().ok()).unwrap_or(4).clamp(1, 100)
}

/// A schema.org Recipe from a page's JSON-LD — which nearly every recipe site
/// publishes for search engines, and which carries exactly the recipe without
/// the story above it.
pub fn extract(html: &str) -> Option<Draft> {
    for block in ld_blocks(html) {
        let Ok(v) = serde_json::from_str::<Value>(block.trim()) else { continue };
        let Some(r) = find_recipe(&v) else { continue };
        let title = r.get("name").and_then(text_of).unwrap_or_default();
        let ingredients = texts(r.get("recipeIngredient").or_else(|| r.get("ingredients")));
        if title.is_empty() && ingredients.is_empty() {
            continue;
        }
        let total = r.get("totalTime").and_then(Value::as_str).map(iso_minutes).unwrap_or(0);
        let parts: i64 = ["prepTime", "cookTime"]
            .iter()
            .filter_map(|k| r.get(*k).and_then(Value::as_str))
            .map(iso_minutes)
            .sum();
        return Some(Draft {
            title,
            image: r.get("image").and_then(text_of).unwrap_or_default(),
            minutes: if total > 0 { total } else { parts },
            servings: servings_of(r.get("recipeYield")),
            category: category_of(r),
            ingredients,
            steps: steps_of(r.get("recipeInstructions")),
        });
    }
    None
}

/// A cookbook page read by tesseract, split the way a page is laid out: the
/// first line is the title, lines that start with a quantity are
/// ingredients, and the rest, joined back into paragraphs, is the method.
///
/// ponytail: a guess from the layout. The draft opens in the editor, so a
/// wrong split is fixed by hand before anything is saved.
pub fn draft_from_text(text: &str) -> Draft {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let mut d = Draft { servings: 4, ..Draft::default() };
    let mut para = String::new();
    for l in lines {
        if l.is_empty() {
            if !para.is_empty() {
                d.steps.push(std::mem::take(&mut para));
            }
            continue;
        }
        if d.title.is_empty() {
            d.title = l.to_string();
            continue;
        }
        let ing = parse_ingredient(l);
        if ing.qty.is_some() && l.len() < 60 && !l.ends_with('.') {
            d.ingredients.push(l.to_string());
            continue;
        }
        if !para.is_empty() {
            para.push(' ');
        }
        para.push_str(l);
    }
    if !para.is_empty() {
        d.steps.push(para);
    }
    d
}

pub fn ocr(path: &std::path::Path) -> Result<Draft> {
    if !tulipix_finances::ocr::available() {
        bail!("reading a photo needs tesseract — install it (e.g. `pacman -S tesseract tesseract-data-eng`)");
    }
    let out = std::process::Command::new("tesseract").arg(path).arg("-").output()?;
    if !out.status.success() {
        bail!("tesseract could not read that picture");
    }
    Ok(draft_from_text(&String::from_utf8_lossy(&out.stdout)))
}

// ── keeping the screen on ───────────────────────────────────────────────────

/// Cook mode holds the screen awake with what the desktop provides:
/// `systemd-inhibit` on Linux, `caffeinate` on macOS. Dropping the child ends
/// the hold.
///
/// ponytail: Windows needs SetThreadExecutionState through a Win32 call; until
/// then its screen may dim while cooking.
pub fn hold_awake() -> Option<tokio::process::Child> {
    let (prog, args): (&str, &[&str]) = if cfg!(target_os = "macos") {
        ("caffeinate", &["-d"])
    } else if cfg!(target_os = "linux") {
        ("systemd-inhibit", &["--what=idle", "--who=Tulipix", "--why=Cooking", "sleep", "86400"])
    } else {
        return None;
    };
    tokio::process::Command::new(prog)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()
}

// ── the phone ───────────────────────────────────────────────────────────────

/// The shopping list over the local network, for a phone in the shop.
///
/// The same shape as `cast_serve`: one listener for the process, one random
/// 128-bit path at a time, nothing else answered. The page is rendered from
/// the database on every request, so it is never stale.
///
/// ponytail: ticks on the phone stay on the phone. Posting them back needs a
/// second route behind the same token.
pub mod phone {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU16, Ordering};

    use anyhow::{Result, anyhow};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    static PORT: AtomicU16 = AtomicU16::new(0);
    static TOKEN: Mutex<String> = Mutex::new(String::new());

    fn token() -> String {
        let mut b = [0u8; 16];
        if getrandom::fill(&mut b).is_err() {
            let n = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            b.copy_from_slice(&n.to_le_bytes());
        }
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// A fresh address for the list; the one before it stops answering.
    pub async fn publish() -> Result<String> {
        let ip = tulipix_transfer::net::interfaces()
            .first()
            .map(|(_, ip)| ip.to_string())
            .ok_or_else(|| anyhow!("this computer has no network address a phone could reach"))?;
        let port = serve().await?;
        let t = token();
        if let Ok(mut g) = TOKEN.lock() {
            *g = t.clone();
        }
        Ok(format!("http://{ip}:{port}/{t}"))
    }

    async fn serve() -> Result<u16> {
        let p = PORT.load(Ordering::SeqCst);
        if p != 0 {
            return Ok(p);
        }
        let listener = TcpListener::bind("0.0.0.0:0").await?;
        let port = listener.local_addr()?.port();
        PORT.store(port, Ordering::SeqCst);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    if let Err(e) = answer(stream).await {
                        tracing::debug!(error = %e, "kitchen: phone request failed");
                    }
                });
            }
            PORT.store(0, Ordering::SeqCst);
        });
        Ok(port)
    }

    async fn answer(mut s: TcpStream) -> Result<()> {
        let mut buf = vec![0u8; 4096];
        let n = s.read(&mut buf).await?;
        let head = String::from_utf8_lossy(&buf[..n]);
        let path = head.split_whitespace().nth(1).unwrap_or("");
        let want = TOKEN.lock().map(|g| g.clone()).unwrap_or_default();
        let (status, body) = if !want.is_empty() && path == format!("/{want}") {
            ("200 OK", crate::api::kitchen::phone_page().await.unwrap_or_else(|_| "<p>The list could not be read.</p>".into()))
        } else {
            ("404 Not Found", "<p>Not here.</p>".to_string())
        };
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        s.write_all(resp.as_bytes()).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ing(q: Option<f64>, u: &str, n: &str, note: &str) -> Ing {
        Ing { qty: q, unit: u.into(), name: n.into(), note: note.into() }
    }

    #[test]
    fn ingredient_lines_come_apart() {
        assert_eq!(parse_ingredient("2 tbsp olive oil"), ing(Some(2.0), "tbsp", "olive oil", ""));
        assert_eq!(parse_ingredient("800g chopped tomatoes"), ing(Some(800.0), "g", "chopped tomatoes", ""));
        assert_eq!(parse_ingredient("1½ cups flour, sifted"), ing(Some(1.5), "cup", "flour", "sifted"));
        assert_eq!(parse_ingredient("1 1/2 tsp cumin"), ing(Some(1.5), "tsp", "cumin", ""));
        assert_eq!(parse_ingredient("3 garlic cloves (crushed)"), ing(Some(3.0), "", "garlic cloves", "crushed"));
        assert_eq!(parse_ingredient("2-3 red peppers"), ing(Some(2.0), "", "red peppers", ""));
        assert_eq!(parse_ingredient("Salt"), ing(None, "", "Salt", ""));
        assert_eq!(parse_ingredient("1 bunch of coriander"), ing(Some(1.0), "bunch", "coriander", ""));
    }

    #[test]
    fn keys_match_across_spellings() {
        assert_eq!(key("garlic cloves"), "garlic");
        assert_eq!(key("Chopped tomatoes"), "tomato");
        assert_eq!(key("large eggs"), "egg");
        assert_eq!(key("red peppers"), "red pepper");
        assert_eq!(key("cherries"), "cherry");
    }

    #[test]
    fn quantities_scale_and_print_like_a_cook_writes() {
        assert_eq!(amount(Some(2.0), "tbsp", 1.5), "3 tbsp");
        assert_eq!(amount(Some(800.0), "g", 0.5), "400 g");
        assert_eq!(amount(Some(1.0), "tsp", 0.5), "½ tsp");
        assert_eq!(amount(Some(1.0), "cup", 1.5), "1½ cup");
        assert_eq!(amount(Some(6.0), "", 2.0 / 3.0), "4");
        assert_eq!(amount(Some(1.0), "", 0.43), "0.4");
        assert_eq!(amount(None, "", 2.0), "");
    }

    #[test]
    fn steps_name_their_timers() {
        assert_eq!(timers("Pour in the tomatoes and simmer for 10 minutes."), vec![("Simmer".into(), 600)]);
        assert_eq!(timers("Bake for 1 hour 20 minutes until golden."), vec![("Bake".into(), 4800)]);
        assert_eq!(timers("Cook 8-10 mins, then rest for 5 minutes"), vec![("Cook".into(), 480), ("Cook".into(), 300)]);
        assert_eq!(timers("Crumble over the feta."), vec![]);
        assert_eq!(timers("Heat 2 tbsp oil"), vec![]);
    }

    #[test]
    fn a_step_knows_which_ingredients_it_uses() {
        let names = vec!["olive oil".to_string(), "chopped tomatoes".to_string(), "eggs".to_string()];
        assert_eq!(uses("Pour in the tomatoes, season, and simmer.", &names), vec![1]);
    }

    #[test]
    fn aisles_prefer_the_longer_word() {
        assert_eq!(aisle("coconut milk"), "Cupboard");
        assert_eq!(aisle("milk"), "Fridge");
        assert_eq!(aisle("red pepper"), "Veg and fruit");
        assert_eq!(aisle("feta"), "Fridge");
        assert_eq!(aisle("widget"), "Other");
    }

    #[test]
    fn the_list_adds_up_like_for_like() {
        let lines = vec![
            ("egg".to_string(), "Eggs".to_string(), Some(6.0), String::new()),
            ("egg".to_string(), "eggs".to_string(), Some(2.0), String::new()),
            ("salt".to_string(), "salt".to_string(), None, String::new()),
            ("rice".to_string(), "rice".to_string(), Some(300.0), "g".to_string()),
        ];
        let m = merge(&lines);
        assert_eq!(m[0], ("egg".to_string(), "Eggs".to_string(), Some(8.0), String::new()));
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn a_page_gives_up_its_recipe() {
        let html = r#"<html><head><script type="application/ld+json">
        {"@context":"https://schema.org","@graph":[{"@type":"WebPage"},
         {"@type":["Recipe"],"name":"Shakshuka &amp; bread","image":[{"url":"https://x/a.jpg"}],
          "recipeYield":["4","4 servings"],"totalTime":"PT35M",
          "recipeIngredient":["2 tbsp olive oil","6 eggs"],
          "recipeCategory":"Breakfast","suitableForDiet":"https://schema.org/VegetarianDiet",
          "recipeInstructions":[{"@type":"HowToSection","itemListElement":[
             {"@type":"HowToStep","text":"Soften the onion."},{"@type":"HowToStep","text":"Add the eggs."}]}]}]}
        </script></head><body>A long story.</body></html>"#;
        let d = extract(html).unwrap();
        assert_eq!(d.title, "Shakshuka & bread");
        assert_eq!(d.image, "https://x/a.jpg");
        assert_eq!((d.minutes, d.servings), (35, 4));
        assert_eq!(d.category, "Vegetarian");
        assert_eq!(d.ingredients.len(), 2);
        assert_eq!(d.steps, vec!["Soften the onion.", "Add the eggs."]);
        assert_eq!(iso_minutes("P0DT1H30M"), 90);
    }

    #[test]
    fn a_photographed_page_splits_into_parts() {
        let d = draft_from_text("Lemon pasta\n\n200g spaghetti\n1 lemon\n\nBoil the pasta.\nToss with the lemon.\n");
        assert_eq!(d.title, "Lemon pasta");
        assert_eq!(d.ingredients, vec!["200g spaghetti", "1 lemon"]);
        assert_eq!(d.steps, vec!["Boil the pasta. Toss with the lemon."]);
    }
}
