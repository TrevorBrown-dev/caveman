//! The rules engine: fx states from the skater's states, data-driven (a JSON file from the trainer).
//!
//! ```json
//! { "rules": [
//!     { "fx": "fx.kickflip", "when": "flip == kickflip", "mode": "pulse", "pulse_ms": 150, "cooldown_ms": 300 },
//!     { "fx": "fx.grind", "when": "grind" },
//!     { "fx": "fx.manual", "when": "manual && !grind", "hold_ms": 250 } ] }
//! ```
//!
//! `when` is an expression over the skater's states ([`INPUTS`], or any state's full name such as
//! `Animation/Dingo/Bool.State.Is.Push`): `&&` `||` `!` (or `and` `or` `not`), `==` `!=` `<` `<=` `>` `>=`, `x in [a, b]`,
//! numbers, `true`/`false`, and the trick names (`kickflip`, `n_kickflip`, `360flip`, ...) and grind names (`fs_50_50`,
//! `bs_crook`, `bf_fs_smith`, ...) as their enum values. A rule whose states are not all readable (no skater: menus,
//! loading) is off.
//!
//! Modes: `level` (default): on while the condition holds, after `delay_ms` of it, and `hold_ms` longer; `pulse`: on
//! for `pulse_ms` (default 150) at each rise of the condition (`"edge": "fall"`: each fall). `cooldown_ms`: the least
//! time between two turn-ons. Several rules may drive one fx state (any of them on turns it on), and the trainer can
//! force any fx state on for a while ([`Engine::fire`]).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// The kind of value a state holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Bool,
    Int,
    Float,
}

/// The skater's states the rules know by a short name: (name, the state asset's run-time Name, kind). Names verified
/// live on 2026-10-21 (`scratchpad/fxhook/names.json`, watch1.txt).
pub const INPUTS: [(&str, &str, Kind); 14] = [
    ("riding", "Animation/Dingo/Bool.State.OnBoard.Riding", Kind::Bool),
    ("grind", "Animation/Dingo/Bool.State.Is.Grind", Kind::Bool),
    ("grindinto", "Animation/Dingo/Bool.State.Is.GrindInto", Kind::Bool),
    ("manual", "Animation/Dingo/Bool.State.Is.Manual", Kind::Bool),
    ("air", "Animation/dingo/Bool.State.Is.TrickInAir", Kind::Bool),
    ("trick", "Animation/Dingo/Bool.State.Is.Trick", Kind::Bool),
    ("landing", "Animation/dingo/Bool.State.Is.Landing", Kind::Bool),
    ("tricklanding", "Animation/Dingo/Bool.Anim.Is.TrickLanding", Kind::Bool),
    ("powerslide", "Animation/Dingo/Bool.State.Is.Powersliding", Kind::Bool),
    ("wipeout", "Animation/Dingo/Bool.State.Is.Wipeout", Kind::Bool),
    ("grab", "Animation/Dingo/Bool.Anim.IsGrabbing", Kind::Bool),
    ("flip", "Animation/Dingo/EnumGS.Anim.CurrentFlipTrick", Kind::Int),
    ("grindtype", "Animation/Dingo/Enum.GS.Anim.CurrentGrindType", Kind::Int),
    ("speed", "Animation/Dingo/Float.Anim.SkaterSpeed", Kind::Float),
];

/// The flip tricks (`OnBoardRightGestureType`, the values of `flip`; knowledge/trick-triggers.md), by value.
pub const FLIP_TRICKS: [&str; 59] = [
    "none", "ollie", "kickflip", "heelflip", "popshuvit", "varialkickflip", "inwardheelflip", "fspopshuvit", "varialheelflip",
    "hardflip", "360popshuvit", "360flip", "360inwardheelflip", "fs360popshuvit", "laserflip", "360hardflip", "nollie",
    "n_kickflip", "n_heelflip", "n_popshuvit", "n_hardflip", "n_varialheelflip", "n_fspopshuvit", "n_varialkickflip",
    "n_inwardheelflip", "n_fs360popshuvit", "n_360hardflip", "n_laserflip", "n_360popshuvit", "n_360flip", "n_360inwardheelflip",
    "olliepop", "n_olliepop", "wallie", "fswallie", "bswallie", "impossible", "frontfoot_impossible", "n_impossible",
    "n_backfoot_impossible", "lateflip", "heldfliptrick", "bonedollie", "kickflipunderflip", "heelflipunderflip",
    "bspopshuvitunderflip", "fspopshuvitunderflip", "nolliekickflipunderflip", "nollieheelflipunderflip",
    "nolliebspopshuvitunderflip", "nolliefspopshuvitunderflip", "hospitalflip", "flowerflip", "nolliehospitalflip",
    "nollieflowerflip", "impossibleunderflip", "frontfootimpossibleunderflip", "nollieimpossibleunderflip",
    "nolliebackfootimpossibleunderflip",
];

/// The grinds (`Enum.SkateGrindType`, the values of `grindtype`): base 1..17; FS = base, BS = base + 17,
/// BF_FS = base + 34, BF_BS = base + 51.
pub const GRINDS: [&str; 17] = [
    "lip", "board", "50_50", "noseslide", "tailslide", "nosegrind", "5_0", "salad", "smith", "feeble", "willy", "overwilly",
    "noseblunt", "blunt", "crook", "overcrook", "darkslide",
];

/// The value of a trick or grind name (lower case), if it is one.
pub fn constant(name: &str) -> Option<f64> {
    let n = name.to_ascii_lowercase();
    match n.as_str() {
        "true" => return Some(1.0),
        "false" => return Some(0.0),
        _ => {}
    }
    if let Some(i) = FLIP_TRICKS.iter().position(|t| *t == n) {
        return Some(i as f64);
    }
    for (prefix, add) in [("bf_fs_", 34), ("bf_bs_", 51), ("fs_", 0), ("bs_", 17)] {
        if let Some(rest) = n.strip_prefix(prefix) {
            if let Some(i) = GRINDS.iter().position(|g| *g == rest) {
                return Some((i + 1 + add) as f64);
            }
        }
    }
    None
}

/// The input a name means: one of [`INPUTS`] by short name, or a state by its full name (`Animation/...`, any case).
/// Returns (the state's Name as the game writes it, or as given, and its kind; None for a full name: decided when the
/// asset is found).
pub fn input_for(name: &str) -> Option<(String, Option<Kind>)> {
    if let Some((_, full, kind)) = INPUTS.iter().find(|(short, _, _)| short.eq_ignore_ascii_case(name)) {
        return Some((full.to_string(), Some(*kind)));
    }
    if let Some((_, full, kind)) = INPUTS.iter().find(|(_, full, _)| full.eq_ignore_ascii_case(name)) {
        return Some((full.to_string(), Some(*kind)));
    }
    let prefix = "animation/";
    if name.len() > prefix.len() && name.get(..prefix.len()).is_some_and(|p| p.eq_ignore_ascii_case(prefix)) {
        return Some((name.to_string(), None));
    }
    None
}

/// The fx state name a rule or command means: lower case, `fx.` in front (`Kickflip` -> `fx.kickflip`).
pub fn fx_key(name: &str) -> String {
    let n = name.trim().to_ascii_lowercase();
    let n = n.strip_prefix("animation/caveman/").unwrap_or(&n).to_string();
    if n.starts_with("fx.") {
        n
    } else {
        format!("fx.{n}")
    }
}

// ---------------------------------------------------------------- expressions

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Op(&'static str),
}

fn lex(text: &str) -> Result<Vec<Tok>, String> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let ident_char = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || c == b'/';
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let two = if i + 1 < b.len() { [b[i], b[i + 1]] } else { [0, 0] };
        if let Some(op) = ["==", "!=", "<=", ">=", "&&", "||"].into_iter().find(|o| o.as_bytes() == two) {
            out.push(Tok::Op(op));
            i += 2;
            continue;
        }
        if let Some(op) = ["<", ">", "!", "(", ")", "[", "]", ",", "-"].into_iter().find(|o| o.as_bytes()[0] == c) {
            out.push(Tok::Op(op));
            i += 1;
            continue;
        }
        if c == b'=' {
            return Err(format!("\"=\" at {i}: compare with \"==\""));
        }
        if ident_char(c) {
            let start = i;
            while i < b.len() && ident_char(b[i]) {
                i += 1;
            }
            let word = &text[start..i]; // ASCII only: a char boundary at both ends
            match word.parse::<f64>() {
                Ok(v) if c.is_ascii_digit() || c == b'.' => out.push(Tok::Num(v)),
                _ => out.push(Tok::Ident(word.to_string())),
            }
            continue;
        }
        return Err(format!("unexpected {:?} at {i}", c as char));
    }
    Ok(out)
}

/// A compiled condition. Inputs are indices into the engine's input list.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Num(f64),
    Input(usize),
    Not(Box<Expr>),
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Cmp(&'static str, Box<Expr>, Box<Expr>),
    In(Box<Expr>, Vec<f64>, bool),
}

impl Expr {
    fn eval(&self, v: &[Option<f64>]) -> f64 {
        let truth = |b: bool| if b { 1.0 } else { 0.0 };
        match self {
            Expr::Num(x) => *x,
            Expr::Input(i) => v.get(*i).copied().flatten().unwrap_or(0.0),
            Expr::Not(e) => truth(e.eval(v) == 0.0),
            Expr::And(es) => truth(es.iter().all(|e| e.eval(v) != 0.0)),
            Expr::Or(es) => truth(es.iter().any(|e| e.eval(v) != 0.0)),
            Expr::Cmp(op, a, b) => {
                let (a, b) = (a.eval(v), b.eval(v));
                truth(match *op {
                    "==" => a == b,
                    "!=" => a != b,
                    "<" => a < b,
                    "<=" => a <= b,
                    ">" => a > b,
                    _ => a >= b,
                })
            }
            Expr::In(e, list, negate) => {
                let x = e.eval(v);
                truth(list.contains(&x) != *negate)
            }
        }
    }

    fn inputs(&self, out: &mut Vec<usize>) {
        match self {
            Expr::Input(i) => {
                if !out.contains(i) {
                    out.push(*i)
                }
            }
            Expr::Not(e) => e.inputs(out),
            Expr::And(es) | Expr::Or(es) => es.iter().for_each(|e| e.inputs(out)),
            Expr::Cmp(_, a, b) => {
                a.inputs(out);
                b.inputs(out);
            }
            Expr::In(e, _, _) => e.inputs(out),
            Expr::Num(_) => {}
        }
    }
}

struct Parser<'a> {
    toks: Vec<Tok>,
    at: usize,
    /// Resolves an identifier to an input index (adding it), or None.
    resolve: &'a mut dyn FnMut(&str) -> Option<usize>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }

    fn is_word(&self, w: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(s)) if s.eq_ignore_ascii_case(w))
    }

    fn eat_op(&mut self, op: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Op(o)) if *o == op) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn or(&mut self) -> Result<Expr, String> {
        let mut parts = vec![self.and()?];
        while self.eat_op("||") || (self.is_word("or") && {
            self.at += 1;
            true
        }) {
            parts.push(self.and()?);
        }
        Ok(if parts.len() == 1 { parts.pop().unwrap() } else { Expr::Or(parts) })
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut parts = vec![self.not()?];
        while self.eat_op("&&") || (self.is_word("and") && {
            self.at += 1;
            true
        }) {
            parts.push(self.not()?);
        }
        Ok(if parts.len() == 1 { parts.pop().unwrap() } else { Expr::And(parts) })
    }

    fn not(&mut self) -> Result<Expr, String> {
        if self.eat_op("!") || (self.is_word("not") && {
            self.at += 1;
            true
        }) {
            return Ok(Expr::Not(Box::new(self.not()?)));
        }
        self.cmp()
    }

    fn cmp(&mut self) -> Result<Expr, String> {
        let left = self.primary()?;
        for op in ["==", "!=", "<=", ">=", "<", ">"] {
            if self.eat_op(op) {
                let right = self.primary()?;
                return Ok(Expr::Cmp(op, Box::new(left), Box::new(right)));
            }
        }
        let negate = if self.is_word("not") && matches!(self.toks.get(self.at + 1), Some(Tok::Ident(s)) if s.eq_ignore_ascii_case("in")) {
            self.at += 1;
            true
        } else {
            false
        };
        if self.is_word("in") {
            self.at += 1;
            if !self.eat_op("[") {
                return Err("\"in\" needs a list: x in [1, 2]".into());
            }
            let mut list = Vec::new();
            loop {
                if self.eat_op("]") {
                    break;
                }
                match self.primary()? {
                    Expr::Num(v) => list.push(v),
                    _ => return Err("an \"in\" list holds numbers or trick names only".into()),
                }
                if !self.eat_op(",") && !matches!(self.peek(), Some(Tok::Op("]"))) {
                    return Err("expected \",\" or \"]\" in the list".into());
                }
            }
            return Ok(Expr::In(Box::new(left), list, negate));
        }
        if negate {
            return Err("\"not\" here needs \"in\"".into());
        }
        Ok(left)
    }

    fn primary(&mut self) -> Result<Expr, String> {
        match self.toks.get(self.at).cloned() {
            Some(Tok::Num(v)) => {
                self.at += 1;
                Ok(Expr::Num(v))
            }
            Some(Tok::Op("-")) => {
                self.at += 1;
                match self.primary()? {
                    Expr::Num(v) => Ok(Expr::Num(-v)),
                    _ => Err("\"-\" goes before a number".into()),
                }
            }
            Some(Tok::Op("(")) => {
                self.at += 1;
                let e = self.or()?;
                if !self.eat_op(")") {
                    return Err("missing \")\"".into());
                }
                Ok(e)
            }
            Some(Tok::Ident(name)) => {
                self.at += 1;
                if let Some(i) = (self.resolve)(&name) {
                    return Ok(Expr::Input(i));
                }
                if let Some(v) = constant(&name) {
                    return Ok(Expr::Num(v));
                }
                Err(format!("unknown name \"{name}\" (a state: {}, or a full Animation/... name; or a trick or grind name)", INPUTS.map(|i| i.0).join(", ")))
            }
            Some(t) => Err(format!("unexpected {t:?}")),
            None => Err("the condition ends too early".into()),
        }
    }
}

/// Parses a condition; `resolve` maps state names to input indices.
pub fn parse(text: &str, resolve: &mut dyn FnMut(&str) -> Option<usize>) -> Result<Expr, String> {
    let toks = lex(text)?;
    if toks.is_empty() {
        return Err("empty condition".into());
    }
    let mut p = Parser { toks, at: 0, resolve };
    let e = p.or()?;
    if p.at != p.toks.len() {
        return Err(format!("unexpected {:?} after the condition", p.toks[p.at]));
    }
    Ok(e)
}

// ---------------------------------------------------------------- rules

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Level,
    Pulse,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    #[default]
    Rise,
    Fall,
}

fn yes() -> bool {
    true
}

/// One rule as written in the JSON.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleSpec {
    pub fx: String,
    pub when: String,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub edge: Edge,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pulse_ms: Option<u64>,
    #[serde(default)]
    pub delay_ms: u64,
    #[serde(default)]
    pub hold_ms: u64,
    #[serde(default)]
    pub cooldown_ms: u64,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A rules file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulesFile {
    #[serde(default)]
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub rules: Vec<RuleSpec>,
}

/// The default pulse length: long enough for the game's per-frame trigger update to see it at 20 fps and up.
pub const PULSE_MS: u64 = 150;

/// The rules the DLL starts with (and `rules/default.json` in the crate): the plan's examples.
pub const DEFAULT_RULES: &str = include_str!("../rules/default.json");

pub fn default_rules() -> RulesFile {
    serde_json::from_str(DEFAULT_RULES).expect("rules/default.json is valid")
}

/// An input the rules read: the state's Name and its kind (None: not known until the asset is found).
#[derive(Clone, Debug, PartialEq)]
pub struct InputDef {
    pub name: String,
    pub kind: Option<Kind>,
}

#[derive(Clone, Debug)]
struct Rule {
    fx: usize,
    cond: Expr,
    inputs: Vec<usize>,
    spec: RuleSpec,
    // run state
    prev: bool,
    since: Option<u64>,
    on_until: u64,
    on: bool,
    last_on: Option<u64>,
    off_at: Option<u64>,
}

/// The fx state names in use, each with a stable index (what the hook's table refers to): names are only ever added.
#[derive(Clone, Debug, Default)]
pub struct FxNames {
    names: Vec<String>,
}

impl FxNames {
    /// The index of an fx state (added if new), or None when all [`crate::hook::MAX_FX`] are taken.
    pub fn index(&mut self, name: &str) -> Option<usize> {
        let key = fx_key(name);
        if let Some(i) = self.names.iter().position(|n| *n == key) {
            return Some(i);
        }
        if self.names.len() >= crate::hook::MAX_FX {
            return None;
        }
        self.names.push(key);
        Some(self.names.len() - 1)
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        let key = fx_key(name);
        self.names.iter().position(|n| *n == key)
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }
}

/// The compiled rules and the manual overrides.
#[derive(Clone, Debug)]
pub struct Engine {
    rules: Vec<Rule>,
    pub inputs: Vec<InputDef>,
    pub file: RulesFile,
    /// Per fx index: forced on until this time (ms).
    overrides: HashMap<usize, u64>,
}

impl Engine {
    /// Compiles a rules file. fx names get indices from `fx` (stable across reloads).
    pub fn compile(file: &RulesFile, fx: &mut FxNames) -> Result<Engine, String> {
        let mut inputs: Vec<InputDef> = Vec::new();
        let mut rules = Vec::new();
        for (n, spec) in file.rules.iter().enumerate() {
            let label = format!("rule {} ({})", n + 1, spec.fx);
            let fx_index = fx.index(&spec.fx).ok_or_else(|| format!("{label}: more than {} fx states", crate::hook::MAX_FX))?;
            let mut resolve = |name: &str| -> Option<usize> {
                let (full, kind) = input_for(name)?;
                if let Some(i) = inputs.iter().position(|d| d.name.eq_ignore_ascii_case(&full)) {
                    return Some(i);
                }
                inputs.push(InputDef { name: full, kind });
                Some(inputs.len() - 1)
            };
            let cond = parse(&spec.when, &mut resolve).map_err(|e| format!("{label}: {e}"))?;
            if spec.mode == Mode::Pulse && spec.pulse_ms == Some(0) {
                return Err(format!("{label}: pulse_ms 0 would never be seen"));
            }
            let mut used = Vec::new();
            cond.inputs(&mut used);
            rules.push(Rule {
                fx: fx_index,
                cond,
                inputs: used,
                spec: spec.clone(),
                prev: false,
                since: None,
                on_until: 0,
                on: false,
                last_on: None,
                off_at: None,
            });
        }
        Ok(Engine { rules, inputs, file: file.clone(), overrides: HashMap::new() })
    }

    /// Forces an fx state on for `ms` from `now` (0 clears it).
    pub fn fire(&mut self, fx: usize, now: u64, ms: u64) {
        if ms == 0 {
            self.overrides.remove(&fx);
        } else {
            self.overrides.insert(fx, now + ms);
        }
    }

    pub fn clear_overrides(&mut self) {
        self.overrides.clear();
    }

    /// Carries the manual overrides of `old` over (rules reloaded while something is forced on).
    pub fn keep_overrides(&mut self, old: &Engine) {
        self.overrides.extend(old.overrides.iter().map(|(k, v)| (*k, *v)));
    }

    /// Milliseconds an override still has on an fx state.
    pub fn override_left(&self, fx: usize, now: u64) -> u64 {
        self.overrides.get(&fx).map_or(0, |&until| until.saturating_sub(now))
    }

    /// One step: `values[i]` is input i's value (None: unreadable). Returns the on fx indices' values in `out`
    /// (indexed by fx; resized as needed).
    pub fn tick(&mut self, now: u64, values: &[Option<f64>], out: &mut Vec<bool>) {
        for v in out.iter_mut() {
            *v = false;
        }
        for r in &mut self.rules {
            let known = r.inputs.iter().all(|&i| values.get(i).copied().flatten().is_some());
            let cond = r.spec.enabled && known && r.cond.eval(values) != 0.0;
            let cooled = |last_on: Option<u64>, off_at: Option<u64>| -> bool {
                let cd = r.spec.cooldown_ms;
                cd == 0
                    || match r.spec.mode {
                        Mode::Pulse => last_on.is_none_or(|t| now >= t + cd),
                        Mode::Level => off_at.is_none_or(|t| now >= t + cd),
                    }
            };
            match r.spec.mode {
                Mode::Pulse => {
                    let edge = match r.spec.edge {
                        Edge::Rise => cond && !r.prev,
                        Edge::Fall => !cond && r.prev && known,
                    };
                    if edge && cooled(r.last_on, r.off_at) {
                        r.on_until = now + r.spec.pulse_ms.unwrap_or(PULSE_MS);
                        r.last_on = Some(now);
                    }
                    r.on = now < r.on_until;
                }
                Mode::Level => {
                    r.since = if cond { Some(r.since.unwrap_or(now)) } else { None };
                    let ready = r.since.is_some_and(|t| now >= t + r.spec.delay_ms);
                    if ready && (r.on || cooled(r.last_on, r.off_at)) {
                        if !r.on {
                            r.last_on = Some(now);
                        }
                        r.on = true;
                        r.on_until = now + r.spec.hold_ms;
                    } else if r.on && now >= r.on_until {
                        r.on = false;
                        r.off_at = Some(now);
                    }
                }
            }
            r.prev = cond;
            if r.on {
                if out.len() <= r.fx {
                    out.resize(r.fx + 1, false);
                }
                out[r.fx] = true;
            }
        }
        self.overrides.retain(|_, until| *until > now);
        for &fx in self.overrides.keys() {
            if out.len() <= fx {
                out.resize(fx + 1, false);
            }
            out[fx] = true;
        }
    }

    /// The fx indices the rules drive.
    pub fn fx_driven(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.rules.iter().map(|r| r.fx).collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine(json: &str) -> (Engine, FxNames) {
        let mut fx = FxNames::default();
        let file: RulesFile = serde_json::from_str(json).unwrap();
        (Engine::compile(&file, &mut fx).unwrap(), fx)
    }

    fn input(e: &Engine, short: &str) -> usize {
        let full = input_for(short).unwrap().0;
        e.inputs.iter().position(|d| d.name == full).unwrap()
    }

    #[test]
    fn constants() {
        assert_eq!(constant("Kickflip"), Some(2.0));
        assert_eq!(constant("n_hardflip"), Some(20.0));
        assert_eq!(constant("wallie"), Some(33.0));
        assert_eq!(constant("360flip"), Some(11.0));
        assert_eq!(constant("nolliebackfootimpossibleunderflip"), Some(58.0));
        assert_eq!(constant("fs_50_50"), Some(3.0));
        assert_eq!(constant("bs_50_50"), Some(20.0));
        assert_eq!(constant("bf_bs_lip"), Some(52.0));
        assert_eq!(constant("bs_darkslide"), Some(34.0));
        assert_eq!(constant("bf_fs_darkslide"), Some(51.0));
        assert_eq!(constant("nope"), None);
        assert_eq!(fx_key("Kickflip"), "fx.kickflip");
        assert_eq!(fx_key("fx.Trick.Any"), "fx.trick.any");
        assert_eq!(fx_key("Animation/Caveman/FX.Grind"), "fx.grind");
    }

    #[test]
    fn expressions() {
        let mut names: Vec<String> = Vec::new();
        let mut resolve = |n: &str| -> Option<usize> {
            let (full, _) = input_for(n)?;
            if let Some(i) = names.iter().position(|x| *x == full) {
                return Some(i);
            }
            names.push(full);
            Some(names.len() - 1)
        };
        let e = parse("flip == kickflip", &mut resolve).unwrap();
        assert_eq!(e, Expr::Cmp("==", Box::new(Expr::Input(0)), Box::new(Expr::Num(2.0))));
        let e = parse("grind && !(manual or flip in [2, heelflip, 360flip])", &mut resolve).unwrap();
        assert_eq!(e.eval(&[Some(0.0), Some(1.0), Some(0.0)]), 1.0); // flip 0, grind 1, manual 0
        assert_eq!(e.eval(&[Some(3.0), Some(1.0), Some(0.0)]), 0.0); // heelflip
        assert_eq!(e.eval(&[Some(0.0), Some(1.0), Some(1.0)]), 0.0); // manual
        let e = parse("speed >= 2.5 and grindtype not in [fs_50_50, bs_50_50]", &mut resolve).unwrap();
        let mut v = vec![None; 5]; // flip, grind, manual, speed, grindtype
        v[3] = Some(3.0);
        v[4] = Some(20.0);
        assert_eq!(e.eval(&v), 0.0);
        v[4] = Some(7.0);
        assert_eq!(e.eval(&v), 1.0);
        let e = parse("Animation/Dingo/Bool.State.Is.Push", &mut resolve).unwrap();
        assert_eq!(e, Expr::Input(5));
        assert!(parse("flip = 2", &mut resolve).unwrap_err().contains("=="));
        assert!(parse("flip == ", &mut resolve).is_err());
        assert!(parse("kickflop", &mut resolve).unwrap_err().contains("unknown name"));
        assert!(parse("(grind", &mut resolve).is_err());
        assert!(parse("grind manual", &mut resolve).is_err());
        assert_eq!(parse("-1 < 0", &mut resolve).unwrap().eval(&[]), 1.0);
    }

    #[test]
    fn level_hold_delay_cooldown() {
        let (mut e, fx) = engine(r#"{"rules": [{"fx": "grind", "when": "grind", "delay_ms": 50, "hold_ms": 100, "cooldown_ms": 500}]}"#);
        let g = input(&e, "grind");
        let f = fx.find("fx.grind").unwrap();
        let mut out = Vec::new();
        let mut at = |e: &mut Engine, t: u64, on: f64| {
            let mut v = vec![None; e.inputs.len()];
            v[g] = Some(on);
            e.tick(t, &v, &mut out);
            out.get(f).copied().unwrap_or(false)
        };
        assert!(!at(&mut e, 0, 1.0)); // delay
        assert!(!at(&mut e, 40, 1.0));
        assert!(at(&mut e, 50, 1.0)); // on after 50 ms of the condition
        assert!(at(&mut e, 100, 0.0)); // held
        assert!(at(&mut e, 149, 0.0));
        assert!(!at(&mut e, 150, 0.0)); // off 100 ms after the condition ended
        assert!(!at(&mut e, 200, 1.0)); // cooldown (500 ms from the turn-off) and delay
        assert!(!at(&mut e, 640, 1.0));
        assert!(at(&mut e, 650, 1.0));
    }

    #[test]
    fn pulse_edges_and_cooldown() {
        let (mut e, fx) = engine(
            r#"{"rules": [{"fx": "fx.kickflip", "when": "flip == kickflip", "mode": "pulse", "pulse_ms": 100, "cooldown_ms": 300},
                          {"fx": "fx.landed", "when": "flip != 0", "mode": "pulse", "edge": "fall"}]}"#,
        );
        let flip = input(&e, "flip");
        let (k, l) = (fx.find("kickflip").unwrap(), fx.find("landed").unwrap());
        let mut out = Vec::new();
        let mut step = |e: &mut Engine, t: u64, v: Option<f64>| {
            let mut vals = vec![None; e.inputs.len()];
            vals[flip] = v;
            e.tick(t, &vals, &mut out);
            (out.get(k).copied().unwrap_or(false), out.get(l).copied().unwrap_or(false))
        };
        assert_eq!(step(&mut e, 0, Some(0.0)), (false, false));
        assert_eq!(step(&mut e, 10, Some(2.0)), (true, false)); // rise
        assert_eq!(step(&mut e, 60, Some(2.0)), (true, false));
        assert_eq!(step(&mut e, 110, Some(2.0)), (false, false)); // 100 ms pulse over while the trick goes on
        assert_eq!(step(&mut e, 200, Some(0.0)), (false, true)); // fall: landed pulse (150 ms default)
        assert_eq!(step(&mut e, 250, Some(2.0)), (false, true)); // a second kickflip inside the 300 ms cooldown
        assert_eq!(step(&mut e, 360, Some(0.0)), (false, true)); // its landing: a new landed pulse (no cooldown there)
        assert_eq!(step(&mut e, 400, Some(2.0)), (true, true)); // a kickflip after the cooldown
        assert_eq!(step(&mut e, 450, None), (true, true)); // no skater: the pulses run out, no fall pulse
        assert_eq!(step(&mut e, 520, None), (false, false));
    }

    #[test]
    fn overrides_and_unknown_inputs() {
        let (mut e, mut fx) = engine(r#"{"rules": [{"fx": "fx.trick.any", "when": "trick"}]}"#);
        let t = fx.index("fx.test").unwrap();
        let a = fx.find("trick.any").unwrap();
        let mut out = Vec::new();
        e.fire(t, 1000, 500);
        e.tick(1000, &[None], &mut out);
        assert!(out[t] && !out[a]); // unknown input: rule off; override on
        e.tick(1400, &[Some(1.0)], &mut out);
        assert!(out[t] && out[a]);
        assert_eq!(e.override_left(t, 1400), 100);
        e.tick(1500, &[Some(0.0)], &mut out);
        assert!(!out[t] && !out[a]);
    }

    #[test]
    fn default_rules_compile() {
        let mut fx = FxNames::default();
        let e = Engine::compile(&default_rules(), &mut fx).unwrap();
        assert!(fx.find("fx.kickflip").is_some() && fx.find("fx.grind").is_some() && fx.find("fx.trick.any").is_some());
        assert!(e.inputs.iter().all(|d| d.kind.is_some()), "the defaults use known states only");
        assert!(serde_json::from_str::<RulesFile>(r#"{"rules": [{"fx": "x", "when": "grind", "hold": 3}]}"#).is_err(), "unknown fields are refused");
    }
}
