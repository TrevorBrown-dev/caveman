//! `mem ...`: the allocation trace (the DLL's alloctrace.rs): every GPU allocation the game makes for a window of
//! seconds, with its call site, size, time taken, lifetime and how much of it the game used, grouped by call site in
//! the report; the raw lines go to `%LOCALAPPDATA%\Caveman\fx\alloc-trace-<seconds>.jsonl`.

use std::time::{Duration, Instant};

use caveman_fx::proto::Request;
use serde_json::Value;

use crate::board::send;
use crate::{arg_value, Result};

pub const USAGE: &str = "  mem trace [SECONDS] [--min KB]  record every GPU allocation of the game's for SECONDS (30), allocations of
                                 KB (64) and more: respawn, teleport, pause and unpause meanwhile; then the report
  mem status | stop | report     what the trace is doing; end it early; the report of the last trace
  mem ring on|off [--mb N]       the staging ring (the fix for the skater build's memory spike): the game's big
                                 upload buffers from a ring of N MB (1024) of ours, recycled once copied out of";

fn f(v: &Value, k: &str) -> f64 {
    v[k].as_f64().unwrap_or(0.0)
}

fn i(v: &Value, k: &str) -> i64 {
    v[k].as_i64().unwrap_or(0)
}

fn pct(x: f64) -> String {
    if x < 0.0 {
        "?".into()
    } else {
        format!("{:.0}%", x * 100.0)
    }
}

fn print_report(m: &Value) {
    println!(
        "allocation trace: {} allocation(s), {:.0} MB, in {:.0} s ({} dropped past the buffer, allocations of {} KB and more); {} still held at the report; system memory for the GPU {} MB at the start, {} MB at the peak",
        i(m, "allocations"),
        f(m, "total_mb"),
        f(m, "window_s"),
        i(m, "dropped"),
        i(m, "min_bytes") / 1024,
        i(m, "held_at_report"),
        i(&m["system_memory_mb"], "first"),
        i(&m["system_memory_mb"], "peak")
    );
    if let Some(h) = m["by_heap_mb"].as_object() {
        let mut parts: Vec<(String, f64)> = h.iter().map(|(k, v)| (k.clone(), v.as_f64().unwrap_or(0.0))).collect();
        parts.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        println!("  by heap: {}", parts.iter().map(|(k, v)| format!("{v:.0} MB {k}")).collect::<Vec<_>>().join("; "));
    }
    println!("  by call site (the game's nearest two frames), the most bytes first: count x MB (largest; ms each; freed/held, mean life; copied, touched)");
    for s in m["sites"].as_array().into_iter().flatten().take(16) {
        let life = f(s, "mean_life_ms");
        println!(
            "    {:>5} x {:>8.1} MB  (largest {:>6.1} MB; {:>5.1} ms; freed {} held {}, life {}; copied {} of {}, touched {})  {}",
            i(s, "count"),
            f(s, "mb"),
            f(s, "largest_mb"),
            f(s, "mean_ms"),
            i(s, "freed"),
            i(s, "held"),
            if life < 0.0 { "?".to_string() } else { format!("{life:.0} ms") },
            pct(f(s, "copied_frac")),
            i(s, "copied_n"),
            pct(f(s, "touched_frac")),
            s["site"].as_str().unwrap_or("")
        );
        if let Some(st) = s["stack"].as_str().filter(|x| !x.is_empty()) {
            println!("          {st}");
        }
    }
    println!("  timeline (s: allocations, MB made, MB freed | video MB, system memory for the GPU MB):");
    for t in m["timeline"].as_array().into_iter().flatten().take(120) {
        println!(
            "    {:>7.2}: {:>4} {:>8.1} {:>8.1} | {:>6} {:>6}",
            f(t, "t"),
            i(t, "allocs"),
            f(t, "mb"),
            f(t, "freed_mb"),
            i(t, "video_mb"),
            i(t, "system_mb")
        );
    }
    println!("  the largest:");
    for l in m["largest"].as_array().into_iter().flatten().take(12) {
        let life = i(l, "life_ms");
        println!(
            "    {:>6.2} s thread {} {}: {} in {:.1} ms; life {}; copied to {:.1} MB in {} copies; touched {}",
            f(l, "t"),
            i(l, "thread"),
            l["what"].as_str().unwrap_or(""),
            l["describe"].as_str().unwrap_or(""),
            f(l, "ms"),
            if life < 0 { "held".to_string() } else { format!("{life} ms") },
            f(l, "copied_to_mb"),
            i(l, "copies"),
            if f(l, "touched_mb") < 0.0 { "?".to_string() } else { format!("{:.1} MB", f(l, "touched_mb")) }
        );
        if let Some(st) = l["stack"].as_str().filter(|x| !x.is_empty()) {
            println!("          {st}");
        }
    }
    if let Some(p) = m["file"].as_str() {
        println!("  every allocation: {p}");
    }
}

pub fn mem_cmd(args: &[String]) -> Result<()> {
    let none = || Request::Mem { trace: None, min_kb: None, stop: None, report: None, ring: None, ring_mb: None, ring_any: None };
    match args.first().map(|s| s.to_ascii_lowercase()).as_deref() {
        None | Some("status") => {
            let v = send(&none())?;
            println!("trace: {}", v["mem"]);
            println!("ring:  {}", v["ring"]);
            Ok(())
        }
        Some("ring") => {
            let on = match args.get(1).map(|s| s.to_ascii_lowercase()).as_deref() {
                Some("on") => Some(true),
                Some("off") => Some(false),
                None | Some("status") => None,
                Some(other) => return Err(format!("mem ring {other}: on or off")),
            };
            let mb: Option<u64> = arg_value(args, "--mb").and_then(|v| v.parse().ok());
            let v = send(&Request::Mem { trace: None, min_kb: None, stop: None, report: None, ring: on, ring_mb: mb, ring_any: None })?;
            let r = &v["ring"];
            if on == Some(true) && v["copy_hooks"] == false {
                println!("WARNING: the copy hooks do not reach the game's lists yet (the overlay installs them on its next tick); check `mem ring` again in a few seconds");
            }
            println!(
                "staging ring: {}; heap {} MB of {} MB wanted; {} region(s), {} MB used (head {} MB, tail {} MB): {} held, {} copied, {} submitted, {} done; handed {}, recycled {}, peak {} regions / {} MB; copies {}, submits {}; fallbacks: {} too big, {} ring full, {} no heap yet, {} failed; second copies {}{}",
                if r["enabled"] == true { "ON" } else { "off" },
                r["heap_mb"],
                r["mb"],
                r["regions"],
                r["used_mb"],
                r["head_mb"],
                r["tail_mb"],
                r["held"],
                r["copied"],
                r["submitted"],
                r["done"],
                r["handed"],
                r["recycled"],
                r["peak_live"],
                r["peak_used_mb"],
                r["copies"],
                r["submits"],
                r["fallback_big"],
                r["fallback_full"],
                r["fallback_no_heap"],
                r["fallback_failed"],
                r["rereads"],
                r["last"].as_str().filter(|s| !s.is_empty()).map(|s| format!("\n  last: {s}")).unwrap_or_default()
            );
            Ok(())
        }
        Some("stop") => {
            let v = send(&Request::Mem { trace: None, min_kb: None, stop: Some(true), report: None, ring: None, ring_mb: None, ring_any: None })?;
            println!("{}", v["mem"]);
            Ok(())
        }
        Some("report") => {
            let v = send(&Request::Mem { trace: None, min_kb: None, stop: None, report: Some(true), ring: None, ring_mb: None, ring_any: None })?;
            print_report(&v["mem"]);
            Ok(())
        }
        Some("trace") => {
            let seconds: f64 = args.iter().skip(1).filter(|a| !a.starts_with("--")).find_map(|a| a.parse::<f64>().ok()).unwrap_or(30.0).clamp(1.0, 600.0);
            let min_kb: u64 = arg_value(args, "--min").and_then(|v| v.parse().ok()).unwrap_or(64);
            send(&Request::Mem { trace: Some(seconds), min_kb: Some(min_kb), stop: None, report: None, ring: None, ring_mb: None, ring_any: None })?;
            println!("tracing for {seconds:.0} s (allocations of {min_kb} KB and more): respawn, teleport, pause and unpause now");
            let end = Instant::now() + Duration::from_secs_f64(seconds + 1.5);
            while Instant::now() < end {
                let left = end - Instant::now();
                std::thread::sleep(left.min(Duration::from_secs(5)));
                if let Ok(v) = send(&none()) {
                    println!(
                        "  {} allocation(s) so far, {} still held, {:.0} s left",
                        v["mem"]["allocations"],
                        v["mem"]["live"],
                        v["mem"]["seconds_left"].as_f64().unwrap_or(0.0)
                    );
                }
            }
            let v = send(&Request::Mem { trace: None, min_kb: None, stop: None, report: Some(true), ring: None, ring_mb: None, ring_any: None })?;
            print_report(&v["mem"]);
            Ok(())
        }
        Some(other) => Err(format!("mem {other}: not a mem word\n{USAGE}")),
    }
}
