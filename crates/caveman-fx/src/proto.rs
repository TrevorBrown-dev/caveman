//! The trainer's line protocol over the named pipe: one JSON request per line, one JSON answer per line.
//!
//! ```text
//! {"cmd":"status"}                          -> {"ok":true, "phase":"active", "inputs":[...], "fx":[...], ...}
//! {"cmd":"fire","fx":"fx.test","ms":3000}   -> {"ok":true, "fx":"fx.test", "ms":3000}
//! {"cmd":"stop","fx":"fx.test"}             (no fx: every override)
//! {"cmd":"rules","rules":{"rules":[...]}}   -> {"ok":true, "rules":13, "inputs":[...]} or {"ok":false,"error":"rule 2 (fx.grind): ..."}
//! {"cmd":"rules_get"} {"cmd":"rescan"} {"cmd":"ping"} {"cmd":"unload"}
//! ```

use std::io::{BufRead, BufReader, Write};

use serde::{Deserialize, Serialize};

use crate::rules::RulesFile;

/// The pipe in Skate.exe. Anywhere else (tests, a host program) the DLL adds the process id, so a test can never talk
/// to the game's DLL.
pub const PIPE: &str = r"\\.\pipe\caveman-fx";

pub fn pipe_for(game: bool, pid: u32) -> String {
    if game {
        PIPE.to_string()
    } else {
        format!("{PIPE}-{pid}")
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Ping,
    Status,
    /// Forces an fx state on for `ms` (default 2000).
    Fire {
        fx: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ms: Option<u64>,
    },
    /// Ends an override (all of them without `fx`).
    Stop {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fx: Option<String>,
    },
    Rules {
        rules: RulesFile,
    },
    RulesGet,
    Rescan,
    Unload,
}

pub const FIRE_MS: u64 = 2000;

/// Sends one request to a pipe and returns the answer (the pipe is opened for this request only).
pub fn request(pipe: &str, req: &Request, wait_ms: u32) -> Result<serde_json::Value, String> {
    let line = serde_json::to_string(req).map_err(|e| e.to_string())?;
    request_line(pipe, &line, wait_ms)
}

/// Sends a raw request line.
pub fn request_line(pipe: &str, line: &str, wait_ms: u32) -> Result<serde_json::Value, String> {
    let name = crate::win::wide(pipe);
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(wait_ms as u64);
    let file = loop {
        match std::fs::OpenOptions::new().read(true).write(true).open(pipe) {
            Ok(f) => break f,
            Err(e) => {
                if std::time::Instant::now() >= deadline {
                    return Err(match e.raw_os_error() {
                        Some(2) => format!("{pipe} is not there (is caveman_fx.dll loaded in the game?)"),
                        _ => format!("{pipe}: {e}"),
                    });
                }
                // busy (another client) or not up yet: wait a little
                unsafe { crate::win::WaitNamedPipeW(name.as_ptr(), 200) };
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    };
    let mut writer = &file;
    writer.write_all(line.trim_end().as_bytes()).map_err(|e| e.to_string())?;
    writer.write_all(b"\n").map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    BufReader::new(&file).read_line(&mut answer).map_err(|e| e.to_string())?;
    if answer.trim().is_empty() {
        return Err("no answer (the DLL closed the pipe)".into());
    }
    serde_json::from_str(answer.trim()).map_err(|e| format!("bad answer {answer:?}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip() {
        let r: Request = serde_json::from_str(r#"{"cmd":"fire","fx":"fx.test","ms":3000}"#).unwrap();
        assert_eq!(r, Request::Fire { fx: "fx.test".into(), ms: Some(3000) });
        assert_eq!(serde_json::to_string(&Request::Status).unwrap(), r#"{"cmd":"status"}"#);
        let r: Request = serde_json::from_str(r#"{"cmd":"rules","rules":{"rules":[{"fx":"a","when":"grind"}]}}"#).unwrap();
        assert!(matches!(r, Request::Rules { .. }));
        assert!(serde_json::from_str::<Request>(r#"{"cmd":"explode"}"#).is_err());
        assert_eq!(pipe_for(false, 12), r"\\.\pipe\caveman-fx-12");
    }
}
