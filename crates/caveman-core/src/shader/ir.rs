//! Text matching on DXIL disassembly, reproducing exactly what the regular expressions of `shaderpatch.py` and
//! `nyanpatch.py` match (no regex crate). Each function names the pattern it stands for.

/// `f32(x)` of shaderpatch: an LLVM IR float literal, the bits of the double holding `x` rounded to a float
/// (`struct.pack('<f', x)`), as 16 upper-case hex digits.
pub fn f32_lit(x: f64) -> String {
    format!("0x{:016X}", ((x as f32) as f64).to_bits())
}

/// Python's `\s` for ASCII text.
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c | 0x1c..=0x1f)
}

/// Python's `\w` for ASCII text.
fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Every non-overlapping occurrence of `needle` in `hay`, left to right.
fn occurrences<'a>(hay: &'a str, needle: &'a str) -> impl Iterator<Item = usize> + 'a {
    hay.match_indices(needle).map(|(i, _)| i)
}

/// A small cursor over the bytes after a fixed prefix.
struct Cursor<'a> {
    s: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn lit(&mut self, text: &str) -> Option<()> {
        let t = text.as_bytes();
        if self.s.get(self.at..self.at + t.len()) == Some(t) {
            self.at += t.len();
            Some(())
        } else {
            None
        }
    }

    /// `\d`: exactly one digit.
    fn digit(&mut self) -> Option<u32> {
        let c = *self.s.get(self.at)?;
        if c.is_ascii_digit() {
            self.at += 1;
            Some(u32::from(c - b'0'))
        } else {
            None
        }
    }

    /// `\d+`: one or more digits, as text.
    fn digits(&mut self) -> Option<&'a str> {
        let start = self.at;
        while self.s.get(self.at).is_some_and(|c| c.is_ascii_digit()) {
            self.at += 1;
        }
        (self.at > start).then(|| std::str::from_utf8(&self.s[start..self.at]).unwrap_or(""))
    }

    /// `[^)]+` followed by `\)`: the text up to the next `)`, which is consumed.
    fn until_paren(&mut self) -> Option<&'a str> {
        let rest = &self.s[self.at..];
        let n = rest.iter().position(|&c| c == b')')?;
        if n == 0 {
            return None;
        }
        let out = std::str::from_utf8(&rest[..n]).ok()?;
        self.at += n + 1;
        Some(out)
    }
}

/// The value name ending right before `at`, `(%[\w.]+)` (or `(%\d+)` with `digits_only`): the run of name characters
/// back from `at`, which must follow a `%` and be non-empty.
fn name_before(s: &str, at: usize, digits_only: bool) -> Option<&str> {
    let b = s.as_bytes();
    let mut i = at;
    while i > 0 && (if digits_only { b[i - 1].is_ascii_digit() } else { is_word(b[i - 1]) || b[i - 1] == b'.' }) {
        i -= 1;
    }
    (i < at && i > 0 && b[i - 1] == b'%').then(|| &s[i - 1..at])
}

/// `shaderpatch._handles`: `(%[\w.]+) = call %dx\.types\.Handle @dx\.op\.createHandle\(i32 57, i8 (\d), i32 \d+,
/// i32 (\d+), i1 false\)` gives {"t0", "cb4", "s0", ...: value name}; a later match of the same key wins.
pub fn handles(ir: &str) -> Result<Vec<(String, String)>, String> {
    const P: &str = " = call %dx.types.Handle @dx.op.createHandle(i32 57, i8 ";
    let mut out: Vec<(String, String)> = Vec::new();
    for q in occurrences(ir, P) {
        let Some(name) = name_before(ir, q, false) else { continue };
        let mut c = Cursor { s: ir.as_bytes(), at: q + P.len() };
        let parsed = (|| {
            let class = c.digit()?;
            c.lit(", i32 ")?;
            c.digits()?;
            c.lit(", i32 ")?;
            let index = c.digits()?;
            c.lit(", i1 false)")?;
            Some((class, index))
        })();
        let Some((class, index)) = parsed else { continue };
        let prefix = ["t", "u", "cb", "s"].get(class as usize).ok_or_else(|| format!("handle class {class} out of range"))?;
        let key = format!("{prefix}{index}");
        out.retain(|(k, _)| *k != key);
        out.push((key, name.to_string()));
    }
    Ok(out)
}

/// A key of `handles`' result.
pub fn handle<'a>(handles: &'a [(String, String)], key: &str) -> Result<&'a str, String> {
    handles.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).ok_or_else(|| format!("no {key} handle"))
}

/// `shaderpatch._art_texture`: the lowest `t<n>` below 32 among the resource table's lines
/// `^;\s+texture\s+(\S+)\s+(\S+)\s+\S+\s+t(\d+)\s+1$` whose type is `f32` and dimension `2d`.
pub fn art_texture(ir: &str) -> Result<String, String> {
    let mut regs = Vec::new();
    for line in ir.split('\n') {
        let b = line.as_bytes();
        if b.first() != Some(&b';') || !b.get(1).is_some_and(|&c| is_space(c)) || b.last().is_some_and(|&c| is_space(c)) {
            continue;
        }
        let tokens: Vec<&str> = line[1..].split(|c: char| c.is_ascii() && is_space(c as u8)).filter(|t| !t.is_empty()).collect();
        if tokens.len() != 6 || tokens[0] != "texture" || tokens[5] != "1" {
            continue;
        }
        let Some(n) = tokens[4].strip_prefix('t') else { continue };
        if n.is_empty() || !n.bytes().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if tokens[1] == "f32" && tokens[2] == "2d" {
            if let Ok(v) = n.parse::<u32>() {
                if v < 32 {
                    regs.push(v);
                }
            }
        }
    }
    regs.into_iter().min().map(|r| format!("t{r}")).ok_or_else(|| "no 2d f32 texture below t32".to_string())
}

/// One `storeOutput` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    /// "f32" or "i32".
    pub kind: String,
    pub operand: String,
    /// Where the match starts (the `call`).
    pub start: usize,
}

/// `shaderpatch.patch`'s stores: `call void @dx\.op\.storeOutput\.(f32|i32)\(i32 5, i32 (\d), i32 0, i8 (\d),
/// (?:float|i32) ([^)]+)\)` as {(row, column): store}; a later match of the same key wins.
pub fn deck_stores(ir: &str) -> Vec<((u32, u32), Store)> {
    const P: &str = "call void @dx.op.storeOutput.";
    let mut out: Vec<((u32, u32), Store)> = Vec::new();
    for q in occurrences(ir, P) {
        let mut c = Cursor { s: ir.as_bytes(), at: q + P.len() };
        let parsed = (|| {
            let kind = if c.lit("f32").is_some() {
                "f32"
            } else {
                c.lit("i32")?;
                "i32"
            };
            c.lit("(i32 5, i32 ")?;
            let row = c.digit()?;
            c.lit(", i32 0, i8 ")?;
            let col = c.digit()?;
            c.lit(", ")?;
            if c.lit("float ").is_none() {
                c.lit("i32 ")?;
            }
            let operand = c.until_paren()?;
            Some(((row, col), Store { kind: kind.to_string(), operand: operand.to_string(), start: q }))
        })();
        if let Some((key, store)) = parsed {
            out.retain(|(k, _)| *k != key);
            out.push((key, store));
        }
    }
    out
}

/// `nyanpatch.patch`'s stores: `call void @dx\.op\.storeOutput\.f32\(i32 5, i32 0, i32 0, i8 (\d), float ([^)]+)\)` as
/// {column: (operand, start)}; a later match of the same column wins.
pub fn target0_stores(ir: &str) -> Vec<(u32, String, usize)> {
    const P: &str = "call void @dx.op.storeOutput.f32(i32 5, i32 0, i32 0, i8 ";
    let mut out: Vec<(u32, String, usize)> = Vec::new();
    for q in occurrences(ir, P) {
        let mut c = Cursor { s: ir.as_bytes(), at: q + P.len() };
        let parsed = (|| {
            let col = c.digit()?;
            c.lit(", float ")?;
            Some((col, c.until_paren()?.to_string()))
        })();
        if let Some((col, operand)) = parsed {
            out.retain(|(k, _, _)| *k != col);
            out.push((col, operand, q));
        }
    }
    out
}

/// `nyanpatch.patch`'s inputs: `(%\d+) = call float @dx\.op\.loadInput\.f32\(i32 4, i32 (\d), i32 0, i8 (\d), i32
/// undef\)` as {(row, column): value}; a later match wins.
pub fn inputs(ir: &str) -> Vec<((u32, u32), String)> {
    const P: &str = " = call float @dx.op.loadInput.f32(i32 4, i32 ";
    let mut out: Vec<((u32, u32), String)> = Vec::new();
    for q in occurrences(ir, P) {
        let Some(name) = name_before(ir, q, true) else { continue };
        let mut c = Cursor { s: ir.as_bytes(), at: q + P.len() };
        let parsed = (|| {
            let row = c.digit()?;
            c.lit(", i32 0, i8 ")?;
            let col = c.digit()?;
            c.lit(", i32 undef)")?;
            Some((row, col))
        })();
        if let Some(key) = parsed {
            out.retain(|(k, _)| *k != key);
            out.push((key, name.to_string()));
        }
    }
    out
}

/// `nyanpatch.patch`'s scalars: `(%\d+) = extractvalue %dx\.types\.CBufRet\.f32 (%\d+), 3` (all, in order).
pub fn cbuf_w(ir: &str) -> Vec<(String, String)> {
    const P: &str = " = extractvalue %dx.types.CBufRet.f32 %";
    let mut out = Vec::new();
    for q in occurrences(ir, P) {
        let Some(name) = name_before(ir, q, true) else { continue };
        let mut c = Cursor { s: ir.as_bytes(), at: q + P.len() };
        let parsed = (|| {
            let load = c.digits()?;
            c.lit(", 3")?;
            Some(format!("%{load}"))
        })();
        if let Some(load) = parsed {
            out.push((name.to_string(), load));
        }
    }
    out
}

/// `nyanpatch.patch`'s loads: `(%\d+) = call %dx\.types\.CBufRet\.f32 @dx\.op\.cbufferLoadLegacy\.f32\(i32 59,
/// %dx\.types\.Handle %\d+, i32 (\d+)\)` as {value: register}; a later match wins.
pub fn cbuf_loads(ir: &str) -> Vec<(String, String)> {
    const P: &str = " = call %dx.types.CBufRet.f32 @dx.op.cbufferLoadLegacy.f32(i32 59, %dx.types.Handle %";
    let mut out: Vec<(String, String)> = Vec::new();
    for q in occurrences(ir, P) {
        let Some(name) = name_before(ir, q, true) else { continue };
        let mut c = Cursor { s: ir.as_bytes(), at: q + P.len() };
        let parsed = (|| {
            c.digits()?;
            c.lit(", i32 ")?;
            let reg = c.digits()?;
            c.lit(")")?;
            Some(reg.to_string())
        })();
        if let Some(reg) = parsed {
            out.retain(|(k, _)| k != name);
            out.push((name.to_string(), reg));
        }
    }
    out
}

/// Where `^(\s*)<target>` (multi-line) matches: (start of the match, start of `target`) for every occurrence of
/// `target` whose preceding run of whitespace reaches back to a line start. As Python's `\s*` can cross newlines,
/// the match starts at the first line start of that run.
pub fn line_anchored(text: &str, target: &str) -> Vec<(usize, usize)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    for p in occurrences(text, target) {
        let mut w = p;
        while w > 0 && is_space(b[w - 1]) {
            w -= 1;
        }
        let s = if w == 0 || b[w - 1] == b'\n' { Some(w) } else { b[w..p].iter().position(|&c| c == b'\n').map(|n| w + n + 1) };
        if let Some(s) = s {
            out.push((s, p));
        }
    }
    out
}

/// `re.search(r'!dx\.valver = !\{!(\d+)\}', text).group(1)`.
pub fn valver(text: &str) -> Option<&str> {
    const P: &str = "!dx.valver = !{!";
    for q in occurrences(text, P) {
        let mut c = Cursor { s: text.as_bytes(), at: q + P.len() };
        if let Some(d) = c.digits() {
            if c.lit("}").is_some() {
                return Some(d);
            }
        }
    }
    None
}

/// `re.sub(rf'^!{node} = !\{{i32 1, i32 \d+\}}$', f'!{node} = !{{i32 1, i32 7}}', text, flags=re.M)`: the validator
/// version node set to 1.7 (what the game's `dxil.dll` validates).
pub fn set_valver_17(text: &str, node: &str) -> String {
    let head = format!("!{node} = !{{i32 1, i32 ");
    let lines: Vec<String> = text
        .split('\n')
        .map(|line| match line.strip_prefix(head.as_str()).and_then(|r| r.strip_suffix('}')) {
            Some(d) if !d.is_empty() && d.bytes().all(|c| c.is_ascii_digit()) => format!("{head}7}}"),
            _ => line.to_string(),
        })
        .collect();
    lines.join("\n")
}

/// Python's `repr` of a list of strings (`['%889', '%890', '%891']`), for notes.
pub fn py_list(items: &[&str]) -> String {
    let inner: Vec<String> = items.iter().map(|s| format!("'{s}'")).collect();
    format!("[{}]", inner.join(", "))
}
