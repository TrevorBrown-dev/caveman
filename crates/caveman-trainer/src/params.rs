//! `board params`, `board param`, `board tint`, `board holotint`, `board foil`: the ridden board's live material
//! parameters, through the DLL's material.rs (ported from ReSkate's own board colouring).

use caveman_fx::material::KNOWN;
use caveman_fx::proto::Request;
use serde_json::Value;

use crate::board::send;
use crate::{arg_value, Result};

fn request(key: Option<String>, values: Option<Vec<f32>>, int: Option<i64>, raw: Option<String>, publish: Option<bool>) -> Request {
    Request::Material { key, values, int, raw, publish }
}

/// `board params [--json]`
pub fn params_cmd(args: &[String]) -> Result<()> {
    let v = send(&request(None, None, None, None, None))?;
    if args.iter().any(|a| a == "--json") {
        println!("{}", serde_json::to_string_pretty(&v["material"]).unwrap_or_default());
        return Ok(());
    }
    print_deck(&v["material"]);
    Ok(())
}

fn print_deck(d: &Value) {
    let mats = d["materials"].as_array().cloned().unwrap_or_default();
    println!(
        "Deck_Mat of the ridden board: item {}, appearance {}, entity {}; {} material instance(s){}",
        d["item"].as_str().unwrap_or("-"),
        d["appearance"].as_str().unwrap_or("-"),
        d["entity"].as_str().unwrap_or("-"),
        mats.len(),
        if d["pending"] == true { " (a recipe is on its way: what is here may be replaced)" } else { "" }
    );
    for m in &mats {
        let params = m["params"].as_array().cloned().unwrap_or_default();
        println!("  material {} (variant {}): {} parameter(s)", m["address"].as_str().unwrap_or("-"), m["variant"], params.len());
        let mut rows: Vec<&Value> = params.iter().collect();
        rows.sort_by_key(|p| (p["name"].is_null(), p["guid8"].as_str().unwrap_or("").to_string()));
        for p in rows {
            println!("    {}", param_line(p));
        }
    }
    println!("names: {}", KNOWN.iter().map(|(_, n, what)| format!("{n} = {what}")).collect::<Vec<_>>().join("; "));
}

fn param_line(p: &Value) -> String {
    let name = p["name"].as_str().unwrap_or("-");
    let floats: Vec<f32> = p["floats"].as_array().map(|a| a.iter().filter_map(|f| f.as_f64()).map(|f| f as f32).collect()).unwrap_or_default();
    let size = p["size"].as_u64().unwrap_or(0);
    let count = p["count"].as_u64().unwrap_or(0);
    let kind = if p["color"] == true {
        "ColorRgb".to_string()
    } else {
        format!("{size} B x{count} type {} ({}, {} B)", p["type"], p["type_hash"].as_str().unwrap_or("-"), p["type_size"])
    };
    let value = if p["color"] == true
        || (size.is_multiple_of(4)
            && size > 0
            && size <= 16
            && !floats.is_empty()
            && floats.iter().all(|f| f.is_finite() && (f.abs() < 1.0e6) && (f.abs() > 1.0e-6 || *f == 0.0)))
    {
        floats.iter().map(|f| format!("{f:.4}")).collect::<Vec<_>>().join(" ")
    } else {
        format!("{} (int {})", p["value_hex"].as_str().unwrap_or(""), p["int"])
    };
    format!(
        "{:<20} {:<13} {:<36} {:<28} flags {} identity {:#06x} node {}",
        p["guid8"].as_str().unwrap_or(""),
        name,
        kind,
        value,
        p["flags"],
        p["identity"].as_u64().unwrap_or(0),
        p["node"].as_str().unwrap_or("")
    )
}

fn print_written(v: &Value, what: &str) {
    let m = &v["material"];
    let n = m["written"].as_array().map_or(0, |a| a.len());
    let after: Vec<String> = m["after"].as_array().map(|a| a.iter().map(param_line).collect()).unwrap_or_default();
    let tick = &m["tick"];
    println!(
        "{what}: written into {n} node(s), published on the engine thread: {} ({}; tick hook: {}, {} call(s), thread {}, last {} ms ago)",
        if m["published"] == true { "yes (and three more times over the next frames)" } else { "NO" },
        m["publish_note"].as_str().unwrap_or(""),
        if tick["installed"] == true { "installed" } else { "not installed" },
        tick["calls"],
        tick["thread"],
        tick["last_ms_ago"]
    );
    for a in after {
        println!("  now: {a}");
    }
}

/// `board param KEY V [V V] | --int N | --raw HEX [--no-publish]`
pub fn param_cmd(args: &[String]) -> Result<()> {
    let usage = "board param KEY V [V V] | --int N | --raw HEX [--no-publish] (KEY: a name of board params, a GUID, or 0x key)";
    let key = args.get(1).filter(|a| !a.starts_with("--")).ok_or(usage)?;
    let values: Vec<f32> =
        args[2..].iter().take_while(|a| !a.starts_with("--")).map(|a| a.parse::<f32>().map_err(|_| format!("{a}: a number"))).collect::<Result<_>>()?;
    let int = arg_value(args, "--int").map(|v| v.parse::<i64>().map_err(|_| format!("--int {v}: an integer"))).transpose()?;
    let raw = arg_value(args, "--raw");
    let publish = args.iter().any(|a| a == "--no-publish").then_some(false);
    let v = send(&request(Some(key.clone()), (!values.is_empty()).then_some(values), int, raw, publish))?;
    print_written(&v, &format!("param {key}"));
    Ok(())
}

/// A colour picked on screen as linear light.
fn linear(hex: &str) -> Result<[f32; 3]> {
    let h = hex.trim_start_matches('#');
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("{hex}: RRGGBB"));
    }
    let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap_or(0) as f32 / 255.0;
    let srgb = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    Ok([srgb(c(0)), srgb(c(2)), srgb(c(4))])
}

/// `board tint RRGGBB|neutral [--gain G]` and `board holotint RRGGBB|R G B`
pub fn tint_cmd(args: &[String], which: &str) -> Result<()> {
    let usage = format!("board {which} RRGGBB | R G B{}", if which == "tint" { " | neutral [--gain G]" } else { "" });
    let first = args.get(1).filter(|a| !a.starts_with("--")).ok_or_else(|| usage.clone())?;
    let gain: f32 = arg_value(args, "--gain").map(|g| g.parse::<f32>().map_err(|_| format!("--gain {g}: a number"))).transpose()?.unwrap_or(1.0);
    let rgb: [f32; 3] = if which == "tint" && first.eq_ignore_ascii_case("neutral") {
        [0.5, 0.5, 0.5]
    } else if args.len() >= 4 && args[1..4].iter().all(|a| a.parse::<f32>().is_ok()) {
        [args[1].parse().unwrap(), args[2].parse().unwrap(), args[3].parse().unwrap()]
    } else {
        let l = linear(first)?;
        // the graphic's tint is neutral at 0.5: white picked = no change
        let scale = if which == "tint" { 0.5 } else { 1.0 } * gain;
        [l[0] * scale, l[1] * scale, l[2] * scale]
    };
    let v = send(&request(Some(which.to_string()), Some(rgb.to_vec()), None, None, None))?;
    print_written(&v, &format!("{which} {:.4} {:.4} {:.4}", rgb[0], rgb[1], rgb[2]));
    Ok(())
}

/// `board foil cellular|frost|pearl|glitter|vertical`
pub fn foil_cmd(args: &[String]) -> Result<()> {
    let usage = "board foil cellular|frost|pearl|glitter|vertical";
    let mode = args.get(1).ok_or(usage)?.to_ascii_lowercase();
    let n: i64 = match mode.as_str() {
        "cellular" => 0,
        "frost" => 1,
        "pearl" => 2,
        "glitter" => 3,
        "vertical" => 4,
        _ => return Err(usage.into()),
    };
    let v = send(&request(Some("foilmode".into()), None, Some(n), None, None))?;
    print_written(&v, &format!("foil {mode} ({n})"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_light() {
        assert_eq!(linear("ffffff").unwrap(), [1.0, 1.0, 1.0]);
        assert_eq!(linear("000000").unwrap(), [0.0, 0.0, 0.0]);
        let g = linear("808080").unwrap();
        assert!((g[0] - 0.2158).abs() < 0.001, "{g:?}");
        assert!(linear("12345").is_err());
        assert!(caveman_fx::material::key_of("holotint").is_ok());
    }
}
