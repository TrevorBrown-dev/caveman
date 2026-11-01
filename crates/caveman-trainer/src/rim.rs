//! `caveman-trainer bars rim ...`: the deck's real edge for the bars' `rim` layout. Read from the player's own
//! install each time (the stock deck mesh, LOD0: caveman-core's `mesh::deck_mesh` and `rim::extract`) and sent to
//! the DLL over the pipe; no game geometry is shipped with Caveman.

use std::path::{Path, PathBuf};

use caveman_core::game::Game;
use caveman_fx::proto::Request;
use caveman_fx::rim::RimPath;
use serde_json::Value;

use crate::{arg_value, send, Result};

pub const USAGE: &str = "\
  bars rim [status]              the deck rim the DLL has (for `bars set layout rim`)
  bars rim load [--game DIR] [--points N]
                                 read the stock deck's edge from the game install (default E:\\Games\\ReSkate; N points
                                 round, default 256) and send it to the DLL, which keeps it for the next load; then
                                 `bars set layout rim` stands the bars on it (rim_out, rim_lift: metres out from the
                                 edge along the deck, and off it along the deck's normal)
  bars rim show [--game DIR] [--points N] [--json]
                                 no game running needed: the rim as read from the install (--json: every point)
  bars rim clear                 forget it (the rim layout then draws the ring)";

fn game_dir(args: &[String]) -> PathBuf {
    PathBuf::from(arg_value(args, "--game").unwrap_or_else(|| caveman_cli::DEFAULT_GAME.to_string()))
}

fn points(args: &[String]) -> Result<usize> {
    match arg_value(args, "--points") {
        Some(n) => n.parse::<usize>().ok().filter(|n| (16..=4096).contains(n)).ok_or_else(|| format!("--points {n}: 16 to 4096")),
        None => Ok(caveman_core::rim::POINTS),
    }
}

/// The stock deck's rim from the install at `game`.
pub fn read(game: &Path, n: usize) -> Result<RimPath> {
    let g = Game::open(game).map_err(|e| format!("the game at {}: {e}", game.display()))?;
    let mesh = caveman_core::mesh::deck_mesh(&g).map_err(|e| e.to_string())?;
    let rim = caveman_core::rim::extract(&mesh, n)?;
    let name = caveman_core::mesh::DECK_MESH.rsplit('/').next().unwrap_or("deck mesh");
    Ok(RimPath { label: format!("{name} LOD0, {} vertices", mesh.positions.len()), points: rim.points, out: rim.out, normal: rim.normal })
}

/// A few lines about a rim: its size, the edge's height in the middle and at the nose and tail, the kicks' tilt.
fn describe(p: &RimPath) -> Vec<String> {
    let n = p.points.len();
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for q in &p.points {
        for k in 0..3 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
    }
    let round: f32 = (0..n).map(|i| (0..3).map(|k| (p.points[(i + 1) % n][k] - p.points[i][k]).powi(2)).sum::<f32>().sqrt()).sum();
    let middle: Vec<f32> = p.points.iter().filter(|q| q[2].abs() < 0.1).map(|q| q[1]).collect();
    let middle = middle.iter().sum::<f32>() / middle.len().max(1) as f32;
    let tilt = |o: [f32; 3]| o[1].clamp(-1.0, 1.0).asin().to_degrees();
    vec![
        format!("{} ({n} points, {:.3} m round)", p.label, round),
        format!(
            "  seen from above {:.3} m across by {:.3} m long (x {:+.3} to {:+.3}, z {:+.3} to {:+.3}; the nose at +z)",
            hi[0] - lo[0],
            hi[2] - lo[2],
            lo[0],
            hi[0],
            lo[2],
            hi[2]
        ),
        format!(
            "  the edge {:.3} m up in the middle, {:.3} m at the nose, {:.3} m at the tail; the way out tilts up {:.0} degrees at the nose, {:.0} at the tail",
            middle,
            p.points[0][1],
            p.points[n / 2][1],
            tilt(p.out[0]),
            tilt(p.out[n / 2])
        ),
    ]
}

fn print_status(v: &Value) {
    let r = &v["rim"];
    if r["loaded"] == true {
        println!(
            "  rim: {} ({} points, {} m round; {} by {} m from above, x {} to {}, y {} to {}, z {} to {}; nose {} m up, tail {} m)",
            r["label"].as_str().unwrap_or("?"),
            r["points"],
            r["length"],
            r["size"][0],
            r["size"][1],
            r["min"][0],
            r["max"][0],
            r["min"][1],
            r["max"][1],
            r["min"][2],
            r["max"][2],
            r["nose_y"],
            r["tail_y"]
        );
    } else {
        println!("  rim: none (the rim layout draws the ring; caveman-trainer bars rim load)");
    }
    if !v["bars"].is_null() {
        println!(
            "  bars layout: {}{}",
            v["bars"]["layout"].as_str().unwrap_or("?"),
            if v["bars"]["layout"] != "rim" { " (bars set layout rim to stand them on the rim)" } else { "" }
        );
    }
}

pub fn rim_cmd(args: &[String]) -> Result<()> {
    match args.first().map(|s| s.to_ascii_lowercase()).as_deref() {
        None | Some("status") => {
            print_status(&send(&Request::Rim { path: None, clear: None })?);
            Ok(())
        }
        Some("load") => {
            let path = read(&game_dir(args), points(args)?)?;
            for line in describe(&path) {
                println!("{line}");
            }
            let v = send(&Request::Rim { path: Some(path), clear: None })?;
            println!("sent to the DLL:");
            print_status(&v);
            Ok(())
        }
        Some("show") => {
            let path = read(&game_dir(args), points(args)?)?;
            if args.iter().any(|a| a == "--json") {
                println!("{}", serde_json::to_string_pretty(&path).map_err(|e| e.to_string())?);
            } else {
                for line in describe(&path) {
                    println!("{line}");
                }
                let n = path.points.len();
                for k in (0..n).step_by((n / 16).max(1)) {
                    let (q, o, nr) = (path.points[k], path.out[k], path.normal[k]);
                    println!(
                        "  {k:4}: at ({:+.4}, {:.4}, {:+.4})  out ({:+.3}, {:+.3}, {:+.3})  normal ({:+.3}, {:+.3}, {:+.3})",
                        q[0], q[1], q[2], o[0], o[1], o[2], nr[0], nr[1], nr[2]
                    );
                }
            }
            Ok(())
        }
        Some("clear") => {
            print_status(&send(&Request::Rim { path: None, clear: Some(true) })?);
            Ok(())
        }
        Some(other) => Err(format!("bars rim {other}? bars rim [status] | load | show | clear")),
    }
}
