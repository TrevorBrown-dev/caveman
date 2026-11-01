//! `pip ...`: the board cam (the DLL's pip.rs and overlay.rs): the skateboard drawn in a corner of the screen, turned
//! as the board under the skater is. `pip load` reads the stock deck, truck and wheel meshes from the player's own
//! install (caveman-core's `mesh`), gives them normals and sends them; the other words set the cam through the bars'
//! settings (`pip_*` keys of `bars set`).

use std::path::PathBuf;

use caveman_core::game::Game;
use caveman_core::mesh::{self, Mesh};
use caveman_fx::pip::{PipMesh, PipPart};
use caveman_fx::proto::Request;
use serde_json::{json, Value};

use crate::board::send;
use crate::{arg_value, Result};

pub const USAGE: &str = "  pip on|off                     the board cam: the skateboard (the deck with the board's live art on its underside,
                                 the trucks, the wheels) drawn in a corner from a fixed viewpoint, turned as the board
                                 under the skater is this frame. The first `on` reads the meshes from the game
                                 (pip load) if the DLL has none
  pip load [--game DIR]          read the stock deck, truck and wheel meshes from the install and send them (kept
                                 for the next load)
  pip size F | margin F | corner tl|tr|bl|br
                                 the square's side as a fraction of the screen's height (0.24), the gap to the corner
                                 (0.02), which corner (tr)
  pip view camera|world          turn the board relative to the game camera's heading (a flip looks as on screen) or
                                 to the world
  pip elevation DEG | azimuth DEG | fov DEG | zoom X
                                 the viewpoint: how high it looks from (28), from which side (0 behind the tail,
                                 positive to the right, -35), its field of view (30) and zoom (1)
  pip art on|off | mirror on|off | flip on|off | backdrop A
                                 the live art on the deck's underside, turned left-right or nose-tail when it lands
                                 wrong; the dark square behind the board (0 none, 0.55)
  pip status                     what is loaded and set";

fn game_dir(args: &[String]) -> PathBuf {
    PathBuf::from(arg_value(args, "--game").unwrap_or_else(|| caveman_cli::DEFAULT_GAME.to_string()))
}

/// Area-weighted vertex normals of a mesh, pointing outward: the triangles' winding decides the face normals' side,
/// and the mesh's signed volume under that winding says whether that side is out (a clockwise-wound mesh, as D3D's
/// front faces are, gives inward normals from the right-hand rule, so they are turned round). Returns the normals and
/// whether they were turned.
fn normals(m: &Mesh) -> (Vec<[f32; 3]>, bool) {
    let mut n = vec![[0.0f32; 3]; m.positions.len()];
    let mut volume = 0.0f64;
    for t in &m.triangles {
        let [a, b, c] = [m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]];
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let f = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        volume += (a[0] as f64 * f[0] as f64 + a[1] as f64 * f[1] as f64 + a[2] as f64 * f[2] as f64) / 6.0;
        for &i in t {
            for k in 0..3 {
                n[i as usize][k] += f[k];
            }
        }
    }
    let turned = volume < 0.0;
    let sign = if turned { -1.0 } else { 1.0 };
    for v in &mut n {
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        *v = if len > 1e-12 { [sign * v[0] / len, sign * v[1] / len, sign * v[2] / len] } else { [0.0, 1.0, 0.0] };
    }
    (n, turned)
}

/// The three meshes as one wire mesh: the deck first (its bounds place the art), then the trucks and the wheels.
pub fn read(game_dir: &std::path::Path) -> Result<(PipMesh, Vec<String>)> {
    let game = Game::open(game_dir).map_err(|e| format!("the game at {}: {e}", game_dir.display()))?;
    let mut notes = Vec::new();
    let mut out = PipMesh { label: String::new(), vertices: Vec::new(), indices: Vec::new(), parts: Vec::new(), bounds: [[0.0; 3]; 2] };
    let mut labels = Vec::new();
    let sources: [(&str, std::result::Result<Mesh, mesh::MeshError>, &str); 3] = [
        ("deck", mesh::deck_mesh(&game), mesh::DECK_MESH),
        ("trucks", mesh::trucks_mesh(&game), mesh::TRUCKS_MESH),
        ("wheels", mesh::wheels_mesh(&game), mesh::WHEELS_MESH),
    ];
    for (kind, m, name) in sources {
        let m = match m {
            Ok(m) => m,
            Err(e) => {
                if kind == "deck" {
                    return Err(format!("the deck mesh: {e}"));
                }
                notes.push(format!("the {kind} mesh was not read ({e}): the cam draws without it"));
                continue;
            }
        };
        let (lo, hi) = m.bounds();
        let (n, turned) = normals(&m);
        let down = n.iter().filter(|v| v[1] < -0.15).count();
        notes.push(format!(
            "{kind}: {} vertices, {} triangles, x {:+.3}..{:+.3} y {:+.3}..{:+.3} z {:+.3}..{:+.3} m; normals {} ({} of them point down)",
            m.positions.len(),
            m.triangles.len(),
            lo[0],
            hi[0],
            lo[1],
            hi[1],
            lo[2],
            hi[2],
            if turned { "turned outward (clockwise winding)" } else { "outward as wound" },
            down
        ));
        if kind == "deck" {
            out.bounds = [lo, hi];
        }
        let base = (out.vertices.len() / 6) as u32;
        for (p, nv) in m.positions.iter().zip(&n) {
            out.vertices.extend_from_slice(&[p[0], p[1], p[2], nv[0], nv[1], nv[2]]);
        }
        let first_index = out.indices.len() as u32;
        for t in &m.triangles {
            out.indices.extend_from_slice(&[base + t[0], base + t[1], base + t[2]]);
        }
        out.parts.push(PipPart { kind: kind.into(), first_index, index_count: (m.triangles.len() * 3) as u32 });
        labels.push(name.rsplit('/').next().unwrap_or(name).to_string());
    }
    out.label = format!("{} LOD0", labels.join(" + "));
    out.check()?;
    Ok((out, notes))
}

fn set(key: &str, value: Value) -> Result<Value> {
    let mut patch = serde_json::Map::new();
    patch.insert(key.to_string(), value);
    send(&Request::Bars { set: Some(Value::Object(patch)), land: None })
}

fn print_status(v: &Value) {
    let p = &v["pip"];
    let b = &v["bars"];
    if p["loaded"] == true {
        println!(
            "board cam meshes: {} ({} vertices, {} triangles: {}); frames drawn {}, the art bound: {}",
            p["label"].as_str().unwrap_or(""),
            p["vertices"],
            p["triangles"],
            p["parts"]
                .as_array()
                .map(|a| a.iter().map(|x| format!("{} {}", x["kind"].as_str().unwrap_or(""), x["triangles"])).collect::<Vec<_>>().join(", "))
                .unwrap_or_default(),
            p["frames_drawn"],
            if p["art_bound"] == true { "yes" } else { "no" }
        );
    } else {
        println!("board cam meshes: none sent yet (pip load, or pip on)");
    }
    if !b.is_null() {
        println!(
            "board cam: {}; size {} of the screen's height, margin {}, corner {}, view {}; elevation {}, azimuth {}, fov {}, zoom {}; art {} (mirror {}, flip {}), backdrop {}",
            if b["pip"] == true { "ON" } else { "off" },
            b["pip_size"],
            b["pip_margin"],
            b["pip_corner"].as_str().unwrap_or(""),
            b["pip_view"].as_str().unwrap_or(""),
            b["pip_elevation"],
            b["pip_azimuth"],
            b["pip_fov"],
            b["pip_zoom"],
            if b["pip_art"] == true { "on" } else { "off" },
            if b["pip_mirror"] == true { "on" } else { "off" },
            if b["pip_flip"] == true { "on" } else { "off" },
            b["pip_backdrop"]
        );
    }
}

fn on_off(v: &str) -> Result<bool> {
    match v {
        "on" | "true" | "1" => Ok(true),
        "off" | "false" | "0" => Ok(false),
        other => Err(format!("{other}: on or off")),
    }
}

pub fn pip_cmd(args: &[String]) -> Result<()> {
    let arg = |i: usize, what: &str| args.get(i).cloned().ok_or_else(|| format!("pip {} needs {what}", args.first().map(String::as_str).unwrap_or("")));
    match args.first().map(|s| s.to_ascii_lowercase()).as_deref() {
        None | Some("status") => {
            let v = send(&Request::Pip { mesh: None, clear: None, art: None })?;
            print_status(&v);
            Ok(())
        }
        Some("load") => {
            let (mesh, notes) = read(&game_dir(args))?;
            for n in &notes {
                println!("  {n}");
            }
            let v = send(&Request::Pip { mesh: Some(mesh), clear: None, art: None })?;
            print_status(&v);
            Ok(())
        }
        Some("clear") => {
            let v = send(&Request::Pip { mesh: None, clear: Some(true), art: None })?;
            print_status(&v);
            Ok(())
        }
        Some("on") | Some("off") => {
            let on = args[0].eq_ignore_ascii_case("on");
            if on {
                let v = send(&Request::Pip { mesh: None, clear: None, art: None })?;
                if v["pip"]["loaded"] != true {
                    let (mesh, notes) = read(&game_dir(args))?;
                    for n in &notes {
                        println!("  {n}");
                    }
                    send(&Request::Pip { mesh: Some(mesh), clear: None, art: None })?;
                }
            }
            let v = set("pip", json!(on))?;
            let p = send(&Request::Pip { mesh: None, clear: None, art: None })?;
            print_status(&json!({"pip": p["pip"], "bars": v["bars"]}));
            Ok(())
        }
        Some(key @ ("size" | "margin" | "elevation" | "azimuth" | "fov" | "zoom" | "backdrop")) => {
            let value: f64 = arg(1, "a number")?.parse().map_err(|_| format!("pip {key}: a number"))?;
            let v = set(&format!("pip_{key}"), json!(value))?;
            print_status(&json!({"bars": v["bars"]}));
            Ok(())
        }
        Some(key @ ("corner" | "view")) => {
            let value = arg(1, if key == "corner" { "tl, tr, bl or br" } else { "camera or world" })?.to_ascii_lowercase();
            let v = set(&format!("pip_{key}"), json!(value))?;
            print_status(&json!({"bars": v["bars"]}));
            Ok(())
        }
        Some(key @ ("art" | "mirror" | "flip")) => {
            let value = on_off(&arg(1, "on or off")?.to_ascii_lowercase())?;
            let v = set(&format!("pip_{key}"), json!(value))?;
            print_status(&json!({"bars": v["bars"]}));
            Ok(())
        }
        Some(other) => Err(format!("pip {other}: not a pip word\n{USAGE}")),
    }
}
