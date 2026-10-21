//! What a run says: the lines caveman.py prints, or with `--json` one JSON object per line (an `event` field and its
//! data) for the app.

use std::io::Write;
use std::path::Path;

use serde_json::{json, Value};

use crate::names::{file_name, Kind};

pub struct Reporter {
    json: bool,
    out: Box<dyn Write + Send>,
}

fn kind_name(kind: &Kind) -> &'static str {
    match kind {
        Kind::Deck => "deck",
        Kind::Wheel => "wheel",
    }
}

impl Reporter {
    /// Lines on standard output.
    pub fn text() -> Reporter {
        Reporter { json: false, out: Box::new(std::io::stdout()) }
    }

    /// JSON events on standard output.
    pub fn json() -> Reporter {
        Reporter { json: true, out: Box::new(std::io::stdout()) }
    }

    /// Either, into any writer (tests).
    pub fn to(out: Box<dyn Write + Send>, json: bool) -> Reporter {
        Reporter { json, out }
    }

    pub fn is_json(&self) -> bool {
        self.json
    }

    /// A line in text mode, the event in JSON mode.
    pub fn say(&mut self, text: &str, event: Value) {
        let line = if self.json { event.to_string() } else { text.to_string() };
        let _ = writeln!(self.out, "{line}");
        let _ = self.out.flush();
    }

    /// A progress line (the build's own log).
    pub fn log(&mut self, text: &str) {
        self.say(text, json!({"event": "log", "message": text}));
    }

    /// A file left out, and why.
    pub fn skipped(&mut self, kind: Kind, path: &Path, reason: &str) {
        let text = format!("  skipped {}: {reason}", file_name(path));
        self.say(&text, json!({"event": "skipped", "kind": kind_name(&kind), "source": path.display().to_string(), "reason": reason}));
    }

    /// A deck or wheel left out because of its name.
    pub fn warning(&mut self, kind: Kind, display: &str, path: &Path, message: &str) {
        let text = format!("  WARNING {display}: {message}");
        self.say(
            &text,
            json!({"event": "warning", "kind": kind_name(&kind), "display": display, "source": path.display().to_string(), "message": message}),
        );
    }

    /// A deck or wheel ready to build.
    pub fn item(&mut self, kind: Kind, display: &str, key: &str, path: &Path, notes: &[String], extra: Value) {
        let text = if notes.is_empty() { format!("  {display}") } else { format!("  {display}  ({})", notes.join("; ")) };
        let mut event = json!({
            "event": "item", "kind": kind_name(&kind), "display": display, "key": key,
            "source": path.display().to_string(), "notes": notes,
        });
        if let (Value::Object(e), Value::Object(x)) = (&mut event, extra) {
            e.extend(x);
        }
        self.say(&text, event);
    }
}
