//! `caveman-trainer board ...`: The Caveman, one deck customised live (docs/fx-hook-plan.md, "The Caveman"). The
//! trainer fits a PNG as Caveman fits deck art and encodes the texture data exactly as a deck build would
//! (caveman-core `fxmod::board_art`); the DLL only uploads it into The Caveman's own texture. Effects: the fx.cave.*
//! states, switched on, off or pulsed. Each command is one pipe request (`proto::Request::Board`), so the Caveman app
//! can send the same.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use caveman_core::effects::{self, Usage};
use caveman_core::fxmod::{self, CAVE};
use caveman_core::game::Game;
use caveman_core::image::Image;
use caveman_core::{bc1, build};
use caveman_fx::board as dll_board;
use caveman_fx::proto::{self, BoardArt, Request};
use caveman_fx::win::{self, Handle};
use serde_json::Value;

use crate::{arg_value, check_build, find_game, modules, Result};

pub const USAGE: &str = "  board [status] [--json]        The Caveman (\"000 - The Caveman\", in Caveman-FX): found? equipped? its texture's
                                 state, the art on it, its effects (fx.cave.*)
  board art FILE.png [--colour RRGGBB] [--no-keep] [--game DIR]
                                 put a picture on The Caveman, fitted as Caveman fits deck art (colour behind
                                 transparent parts, else natural wood); kept for a texture made again unless --no-keep
  board deck PATH|NAME [--colour RRGGBB] [--no-keep] [--game DIR]
                                 the same with a deck of the library (<game>\\Caveman\\Decks and \\Imports): a path, or
                                 part of a file name; its colour as Caveman reads it (Name.ff0000.png, settings)
  board reset                    plain white again (The Caveman's own texture data), nothing kept
  board finish KIND [--spot] [--emboss N] [--art FILE] [--game DIR] [--no-keep]
                                 the deck's finish, live: gloss, matte, foil, chrome, glitter, holo (holo-cellular,
                                 -frost, -vertical, -glitter name the pattern: set live through board foil), or
                                 none. The mask, normal map and holo map are built as the build builds them and
                                 written into The Caveman's own layer textures (the holo host, since 2026-10-22);
                                 --spot puts the finish on the light parts of the art only, --emboss N raises the
                                 print (both need --art FILE, the picture on the board); --strength 0..1 dims the
                                 holo (the holo map's red scales the shader's iridescence)
  board video FILE|obs|camera:NAME [--once] [--rotate cw|ccw|none] [--fit cover|contain] [--fps N] [--size WxH]
                                 video on the in-world board: a file (ffmpeg decodes it at its own pace, looped unless
                                 --once, turned 90 degrees clockwise unless --rotate says otherwise), or OBS Studio's
                                 virtual camera (obs: whatever the OBS scene shows, as it is), or any other camera.
                                 Each frame is fitted to the in-world copy's shape (cover, or contain with black bars),
                                 made into a mip chain and handed to the DLL through a shared memory ring; the DLL
                                 uploads the newest one every present. The menu's board keeps the still art. Ctrl+C
                                 stops; board video stop ends a producer running elsewhere (a detached one, the app's)
                                 through its stop event; board video off stops the DLL's side alone (the kept art
                                 comes back); board video status reports frames, drops and timings
  board params [--json]          the ridden board's live material parameters: every parameter of its Deck_Mat
                                 instances (the tint, the holo's colour, pattern and amount, the metal mask, the
                                 textures), with their keys, types and values, as ReSkate's own board colouring
                                 reads them (ported from it)
  board param KEY V [V V] | --int N | --raw HEX [--no-publish]
                                 write one of them (KEY a name of board params, a GUID, or 0x key), then publish the
                                 materials to the board's meshes with the game's own call: the board changes at once
  board tint RRGGBB|neutral [--gain G]
                                 the graphic's tint (param_graphic_tintcolor; the colour as linear light x 0.5, so
                                 white is neutral and a colour tints the art); neutral puts 0.5 0.5 0.5 back
  board holotint RRGGBB|R G B    the holo's colour (linear light; the holo host's own is about 0.64 1.0 ..)
  board foil cellular|frost|pearl|glitter|vertical
                                 the holo pattern (param_foilmode) live
  board fx list | board fx NAME on|off | board fx NAME pulse [MS]
                                 its effects: on until off, or on for MS (default 150: one burst of a one-shot)
  board state [auto|common|srv|ps|nps|generic|0xNN] [--resource 0x...|--kind composite|main|sibling]
                                 the texture's resource state by hand (auto: what the game's barriers and RenderCore2 say),
                                 for every target, or for one resource or one kind (the others untouched)
  board composite-common on|off  write a composite that never shows a barrier from COMMON when its state is automatic
  board find                     look for The Caveman's texture again now
  board siblings on|off          also write the same-shaped textures next to the main one (off: other decks' graphics)
  board finals on|off            also write the finals: the run-time-made copies of The Caveman's shape with the known
                                 name hashes (the two the game makes at every equip; newest first, 2 per hash); default on
  board worldcopy on|off         also write the world copy: the 512 x 2048 BC1 texture (format code 54, name hash
                                 0x0cb4cea5efea33dd) the game re-fills at every deck equip, with the art's BC1 chain,
                                 and again whenever the game re-fills it (the time from re-fill to re-write is logged);
                                 default on. board worldcopy hashes 0x..[,0x..] sets the name hashes looked for
  board ridden on|off            also write the ridden copy: THE texture the in-world board samples (found 2026-10-22):
                                 the nameless sRGB RGBA8 texture of the deck's shape, full or demoted by the streamer
                                 (512 x 2048, 256 x 1024, 128 x 512), found by the census at every equip and written
                                 with the art fitted to its size; writing it changes the ridden board at once, no
                                 re-equip; default on
  board fill RRGGBB [--alpha N] [--premultiply] [--no-keep]
                                 a plain colour on every target (a quick look at which texture the ridden board shows);
                                 --alpha N (0 to 255, default 255) is the RGBA8 chain's alpha (the composite's)
  board art|deck ... [--alpha art|255|0|invert] [--premultiply]
                                 the RGBA8 chain's alpha: art = the PNG's own (fitted, before the wood or colour goes
                                 behind it), invert = 255 minus it, or a constant; --premultiply scales RGB by it. The
                                 BC7 chain (the main, the finals) stays the opaque fitted art
  board peek --resource 0x... --at X,Y [--mip M] [--n 4]
                                 read an N x N block (1 to 16) at a texel position of a target's mip (its state from
                                 the target's rule) and print the texels as the bytes stored (RGBA8/BGRA8) or the
                                 16-byte blocks (BC7; the position rounded to blocks)
  board target 0xADDR [--state common|srv|0xNN]
                                 add a runtime texture object (its address, from skwatch) as a target by hand: any 2D
                                 shape in the BC7, RGBA8 or BGRA8 family, up to 12 mips; the next board art / deck /
                                 fill fits the picture to its size; with --state, that state is set for it
  board target --resource 0xADDR [--state common|srv|0xNN] [--force]
                                 the same from a raw ID3D12Resource pointer (from board uploads or board hunt); accepted
                                 only with a live owner (a RenderCore2 object or a runtime texture naming it, found by a
                                 scan of the game's memory: a few seconds) and checked again at every write; its state
                                 is the one the last write the game ended on it left it in (board uploads), else --state.
                                 --force: no owner needed when board uploads --deck holds it (the game wrote it within
                                 this equip) and GetDesc passes; a re-equip drops it. THE GAME CAN CRASH if it is gone
  board hunt RRGGBB [--assume srv|common|0xNN] [--tolerance N] [--all] [--owners] [--everything] [--census]
                    [--include-targets] [--json]
                                 --census: every runtime texture object in the game's memory (a scan, seconds), not
                                 only the textures the hooks saw, each owner checked before its read; --include-targets:
                                 render targets too (never a back buffer). Resources the game released are skipped
                                 (their destruction callbacks) and each one read is held until the GPU is done
                                 find the texture the ridden board samples by content: after board fill RRGGBB and a
                                 real re-equip (a deck slot change), every 2D texture the DLL knows (BC7, BC1, BC3,
                                 RGBA8, BGRA8, 64 x 64 up, every slice of an array up to 64) is read at three blocks of
                                 mip 0 and decoded; the hits are those with a texel within N (default 24) per channel of
                                 the colour, as the sRGB bytes given or linearised (a UNORM texture may hold either);
                                 --abort drops a hunt that stopped moving (one stuck for 10 s ends by itself);
                                 both also premultiplied by the texel's alpha; N defaults to 24 (32 for BC1/BC3).
                                 --assume S: textures of unknown state are read from S (never a render or depth
                                 target); a wrong S gives garbage bytes, not a crash
                                 --all also prints the deck-shaped misses with their mean colour; --owners
                                 looks up the hits' owners (a scan, seconds); --everything reads every texture ever
                                 described, not only those seen since the equip (A TEXTURE THE GAME RELEASED SINCE
                                 WOULD REMOVE THE DEVICE: use it only right after a load)
  board slices [--json]          the slices of the tile arrays (256 x 256 texture arrays of a BC format: the pages the
                                 world samples) written since the main was last made, by array, with each slice's writes
  board slices match [--json]    read back the 4 corner blocks of every BC7 slice written since the main was last made,
                                 decode them and find our art's tiles among them (every tile position of mips 0 to 3;
                                 a slice is ours within a mean error of 12 per channel): the slices that hold our deck
  board tiles [--state srv|0xNN] [--json]
                                 write our art's tiles into the matched slices (COPY_DEST from the array's observed
                                 state and back), read the corners back, and keep writing them whenever the game
                                 writes one of them again; --state: the arrays' state by hand (auto: the uploads table's)
  board tiles clear              forget the matched slices (nothing written again)
  board target clear             remove every target added by hand
  board uploads [--deck] [--all] [--owners] [--json]
                                 the writes the game ended lately on any resource (out of COPY_DEST, RENDER_TARGET or
                                 UNORDERED_ACCESS), one line per resource (256 kept, turning over within seconds):
                                 presents ago, how many, the states, its shape (GetDesc); those written regularly
                                 (every frame or so) counted unless --all. --deck: the deck-shaped subset (2D of the
                                 main's size, 64 slots kept for minutes, with each entry's first and last present and
                                 the present the main was last made at). --owners: a scan of the game's memory for
                                 each entry's owner (a few seconds; none = transient, released after use)
  board copies [--deck] [--json] the copies the game recorded lately (CopyTextureRegion, CopyResource; the last 1024,
                                 less than a second of them): presents ago, the list and queue types, source and
                                 destination with their shapes; --deck: the deck ring (8192: the copies with a
                                 deck-shaped source or destination, or one not described yet when recorded; minutes)
  board verify                   read a 4 x 4 block of mips 0 and 2 back from the texture now and compare it with the
                                 art written (the DLL does it by itself right after each write and 2 s later)
  board probe [--pid N]          read-only, no injection: find The Caveman's texture from outside and print the chain";

pub(crate) fn send(req: &Request) -> Result<Value> {
    // art is about 1.9 MB of JSON: the request takes a moment
    let v = proto::request(proto::PIPE, req, 5000)?;
    if v["ok"] == false {
        return Err(v["error"].as_str().unwrap_or("failed").to_string());
    }
    Ok(v)
}

fn req(art: Option<BoardArt>, keep: Option<bool>, state: Option<String>, find: bool, fx: Option<String>, on: Option<bool>, ms: Option<u64>) -> Request {
    Request::Board {
        art,
        keep,
        state,
        find: find.then_some(true),
        fx,
        on,
        ms,
        verify: None,
        arts: None,
        siblings: None,
        state_for: None,
        composite_common: None,
        target: None,
        uploads: None,
        finals: None,
        copies: None,
        peek: None,
        force: None,
        owners: None,
        slices: None,
        slices_state: None,
        hunt: None,
        owners_of: None,
        worldcopy: None,
        worldcopy_hashes: None,
        hunt_abort: None,
    }
}

pub fn board_cmd(args: &[String]) -> Result<()> {
    match args.first().map(|s| s.to_ascii_lowercase()).as_deref() {
        None | Some("status") => {
            let v = send(&Request::board())?;
            if args.iter().any(|a| a == "--json") {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            } else {
                print_board(&v);
            }
            Ok(())
        }
        Some("video") => crate::video::video_cmd(args),
        Some("params") => crate::params::params_cmd(args),
        Some("param") => crate::params::param_cmd(args),
        Some("tint") => crate::params::tint_cmd(args, "tint"),
        Some("holotint") => crate::params::tint_cmd(args, "holotint"),
        Some("foil") => crate::params::foil_cmd(args),
        Some("art") => {
            let file = args.get(1).filter(|a| !a.starts_with("--")).ok_or("board art FILE.png")?;
            let path = PathBuf::from(file);
            let paint = colour_arg(args)?;
            put(&path, paint, args)
        }
        Some("deck") => {
            let query = args.get(1).filter(|a| !a.starts_with("--")).ok_or("board deck PATH|NAME")?;
            let path = find_deck(&game_dir(args), query)?;
            let paint = match colour_arg(args)? {
                Some(c) => Some(c),
                None => caveman_cli::names::deck_paint(&path),
            };
            put(&path, paint, args)
        }
        Some("siblings") => {
            let on = crate::on_off(args.get(1))?;
            let v = send(&Request::Board {
                art: None,
                keep: None,
                state: None,
                find: None,
                fx: None,
                on: None,
                ms: None,
                verify: None,
                arts: None,
                siblings: Some(on),
                state_for: None,
                composite_common: None,
                target: None,
                uploads: None,
                finals: None,
                copies: None,
                peek: None,
                force: None,
                owners: None,
                slices: None,
                slices_state: None,
                hunt: None,
                owners_of: None,
                worldcopy: None,
                worldcopy_hashes: None,
                hunt_abort: None,
            })?;
            println!(
                "siblings (the same-shaped textures next to the main one in a material's table, other decks' graphics as it turned out): {}",
                if v["board"]["siblings_on"] == true { "written too" } else { "not written" }
            );
            Ok(())
        }
        Some("reset") => {
            let chain = fxmod::cave_mips();
            let rgba = rgba_chain(&fxmod::cave_art());
            let mut r = req(Some(BoardArt::new("reset (plain white)", &chain)), Some(false), None, false, None, None, None);
            if let Request::Board { arts, .. } = &mut r {
                *arts = Some(vec![BoardArt::with_format("reset (plain white)", "rgba8", dll_board::LEVELS as u32, &rgba)]);
            }
            let v = send(&r)?;
            wait_upload(&v, &sha(&chain))
        }
        Some("fx") => fx_cmd(&args[1..]),
        Some("state") => {
            let s = args.get(1).filter(|a| !a.starts_with("--")).cloned().unwrap_or_else(|| "auto".into());
            let state_for = match (arg_value(args, "--resource"), arg_value(args, "--kind")) {
                (Some(r), _) => Some(r),
                (None, Some(k)) => Some(format!("kind:{k}")),
                (None, None) => None,
            };
            let mut r = req(None, None, Some(s), false, None, None, None);
            if let Request::Board { state_for: sf, .. } = &mut r {
                *sf = state_for;
            }
            let v = send(&r)?;
            print_board(&v);
            Ok(())
        }
        Some("composite-common") => {
            let on = crate::on_off(args.get(1))?;
            let v = send(&Request::Board {
                art: None,
                keep: None,
                state: None,
                find: None,
                fx: None,
                on: None,
                ms: None,
                verify: None,
                arts: None,
                siblings: None,
                state_for: None,
                composite_common: Some(on),
                target: None,
                uploads: None,
                finals: None,
                copies: None,
                peek: None,
                force: None,
                owners: None,
                slices: None,
                slices_state: None,
                hunt: None,
                owners_of: None,
                worldcopy: None,
                worldcopy_hashes: None,
                hunt_abort: None,
            })?;
            println!(
                "composite-common: {}",
                if v["board"]["composite_common"] == true {
                    "on (a composite that never shows a barrier is written from COMMON when its state is automatic)"
                } else {
                    "off"
                }
            );
            Ok(())
        }
        Some("ridden") => {
            let on = crate::on_off(args.get(1))?;
            let v = send(&Request::Board {
                art: None,
                keep: None,
                state: None,
                find: None,
                fx: None,
                on: None,
                ms: None,
                verify: None,
                arts: None,
                siblings: None,
                state_for: None,
                composite_common: None,
                target: Some(if on { "ridden on".into() } else { "ridden off".into() }),
                uploads: None,
                finals: None,
                copies: None,
                peek: None,
                force: None,
                owners: None,
                slices: None,
                slices_state: None,
                hunt: None,
                owners_of: None,
                worldcopy: None,
                worldcopy_hashes: None,
                hunt_abort: None,
            })?;
            println!(
                "ridden copy: {} (the nameless sRGB RGBA8 texture of the deck's shape, full or demoted, that the in-world board samples; found by the census at every equip); found now: {}",
                if v["board"]["ridden_on"] == true { "looked for and written" } else { "not written" },
                v["board"]["ridden"]["found"]
            );
            Ok(())
        }
        Some("finals") => {
            let on = crate::on_off(args.get(1))?;
            let v = send(&Request::Board {
                art: None,
                keep: None,
                state: None,
                find: None,
                fx: None,
                on: None,
                ms: None,
                verify: None,
                arts: None,
                siblings: None,
                state_for: None,
                composite_common: None,
                target: None,
                uploads: None,
                finals: Some(on),
                copies: None,
                peek: None,
                force: None,
                owners: None,
                slices: None,
                slices_state: None,
                hunt: None,
                owners_of: None,
                worldcopy: None,
                worldcopy_hashes: None,
                hunt_abort: None,
            })?;
            let f = &v["board"]["finals"];
            println!(
                "finals: {} (the run-time-made copies of The Caveman's shape with the name hashes {}; {} per hash, newest first); found now: {}",
                if v["board"]["finals_on"] == true { "written too" } else { "not written" },
                f["hashes"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
                f["per_hash"],
                f["found"]
            );
            Ok(())
        }
        Some("worldcopy") => match args.get(1).map(|a| a.as_str()) {
            Some("hashes") => {
                let list: Vec<String> = args.get(2).map(|l| l.split(',').map(|h| h.trim().to_string()).filter(|h| !h.is_empty()).collect()).unwrap_or_default();
                let v = send(&Request::Board {
                    art: None,
                    keep: None,
                    state: None,
                    find: None,
                    fx: None,
                    on: None,
                    ms: None,
                    verify: None,
                    arts: None,
                    siblings: None,
                    state_for: None,
                    composite_common: None,
                    target: None,
                    uploads: None,
                    finals: None,
                    copies: None,
                    peek: None,
                    force: None,
                    owners: None,
                    slices: None,
                    slices_state: None,
                    hunt: None,
                    owners_of: None,
                    worldcopy: None,
                    worldcopy_hashes: Some(list),
                    hunt_abort: None,
                })?;
                println!("world copy: the name hashes looked for: {}", v["board"]["worldcopies"]["hashes"]);
                Ok(())
            }
            other => {
                let on = crate::on_off(other.map(|s| s.to_string()).as_ref())?;
                let v = send(&Request::Board {
                    art: None,
                    keep: None,
                    state: None,
                    find: None,
                    fx: None,
                    on: None,
                    ms: None,
                    verify: None,
                    arts: None,
                    siblings: None,
                    state_for: None,
                    composite_common: None,
                    target: None,
                    uploads: None,
                    finals: None,
                    copies: None,
                    peek: None,
                    force: None,
                    owners: None,
                    slices: None,
                    slices_state: None,
                    hunt: None,
                    owners_of: None,
                    worldcopy: Some(on),
                    worldcopy_hashes: None,
                    hunt_abort: None,
                })?;
                let w = &v["board"]["worldcopies"];
                println!(
                    "worldcopy: {} (the 512 x 2048 BC1 texture of format code {} with a name hash of {}; found now: {})",
                    if v["board"]["worldcopy_on"] == true { "written too, and again whenever the game re-fills it" } else { "not written" },
                    w["format_code"],
                    w["hashes"],
                    w["found"]
                );
                Ok(())
            }
        },
        Some("fill") => {
            let c = args.get(1).filter(|a| !a.starts_with("--")).ok_or("board fill RRGGBB [--slot osk|ny|rgb]")?;
            let rgb = effects::hex_colour(c.trim_start_matches('#')).ok_or_else(|| format!("fill {c}: RRGGBB"))?;
            let img = Image::filled(dll_board::WIDTH as usize, dll_board::HEIGHT as usize, &rgb);
            let label = format!("fill #{}", c.trim_start_matches('#').to_ascii_lowercase());
            // `--slot`: the colour into one of The Caveman's layers instead (the mask, the normal map, the holo map:
            // a needle for finding the copy the in-world board samples, or the holo map red by hand)
            if let Some(slot) = arg_value(args, "--slot") {
                if !matches!(slot.as_str(), "osk" | "ny" | "rgb") {
                    return Err(format!("--slot {slot}: osk, ny or rgb"));
                }
                let bc7 = build::mip_chain(&img.to_rgb(), dll_board::LEVELS, slot == "ny", caveman_core::bc7::encode);
                // the RGBA8 chain goes into the in-world board's copy of the layer: re-encoded as the game's
                // compositor would (effects::ridden_layer); the BC7 chain is the source, as it is
                let arts = vec![
                    BoardArt::new(&label, &bc7).for_slot(&slot),
                    BoardArt::with_format(&label, "rgba8", dll_board::LEVELS as u32, &rgba_chain(&effects::ridden_layer(&slot, &img))).for_slot(&slot),
                ];
                let keep = !args.iter().any(|a| a == "--no-keep");
                let mut r = req(None, Some(keep), None, false, None, None, None);
                if let Request::Board { arts: a, .. } = &mut r {
                    *a = Some(arts);
                }
                println!("{label} into the {slot} layer (a BC7 chain of 12 levels and an RGBA8 chain)");
                let v = send(&r)?;
                return wait_upload(&v, &sha(&bc7));
            }
            put_image(img, &label, Vec::new(), args, None)
        }
        Some("target") => {
            let what = match (args.get(1).filter(|a| !a.starts_with("--")), arg_value(args, "--resource")) {
                (Some(t), _) => t.clone(),
                (None, Some(r)) => format!("resource:{r}"),
                (None, None) => return Err("board target 0xADDR [--state S] | board target --resource 0xADDR [--state S] | board target clear".into()),
            };
            let state = arg_value(args, "--state");
            let raw = what.starts_with("resource:");
            let force = args.iter().any(|a| a == "--force");
            let v = send(&Request::Board {
                art: None,
                keep: None,
                state,
                find: None,
                fx: None,
                on: None,
                ms: None,
                verify: None,
                arts: None,
                siblings: None,
                state_for: None,
                composite_common: None,
                target: Some(what),
                uploads: None,
                finals: None,
                copies: None,
                peek: None,
                force: force.then_some(true),
                owners: None,
                slices: None,
                slices_state: None,
                hunt: None,
                owners_of: None,
                worldcopy: None,
                worldcopy_hashes: None,
                hunt_abort: None,
            })?;
            let v = if raw && !force {
                // a raw resource needs a live owner: a scan of the game's memory for the pointer (a few seconds)
                println!("looking for a live owner of the resource (a RenderCore2 object or a runtime texture naming it: a scan of the game's memory)...");
                let t = Instant::now();
                loop {
                    std::thread::sleep(Duration::from_millis(500));
                    let v = send(&Request::board())?;
                    if v["board"]["raw_pending"].is_null() || t.elapsed() > Duration::from_secs(90) {
                        println!("{}", v["board"]["raw_last"].as_str().unwrap_or("?"));
                        break v;
                    }
                }
            } else {
                // GetDesc on it lands at the next frame: ask again
                std::thread::sleep(Duration::from_millis(400));
                send(&Request::board()).unwrap_or(v)
            };
            print_board(&v);
            Ok(())
        }
        Some("copies") => {
            send(&Request::Board {
                art: None,
                keep: None,
                state: None,
                find: None,
                fx: None,
                on: None,
                ms: None,
                verify: None,
                arts: None,
                siblings: None,
                state_for: None,
                composite_common: None,
                target: None,
                uploads: None,
                finals: None,
                copies: Some(true),
                peek: None,
                force: None,
                owners: None,
                slices: None,
                slices_state: None,
                hunt: None,
                owners_of: None,
                worldcopy: None,
                worldcopy_hashes: None,
                hunt_abort: None,
            })?;
            // the render thread describes the ring's resources at the next frame
            std::thread::sleep(Duration::from_millis(500));
            let v = send(&Request::board())?;
            let deck = args.iter().any(|a| a == "--deck");
            // --deck: the deck ring (the copies whose source or destination is deck-shaped, or was not described when
            // the copy was recorded: 8192 entries, so an equip stays in it for minutes)
            let ring = if deck { v["board"]["copies_deck"].clone() } else { v["board"]["copies_ring"].clone() };
            if deck {
                let (w, h) = main_size(&v);
                println!("the deck ring (copies with a {w} x {h} 2D source or destination, or one not described yet when recorded; the main was last made at present {}, now {}):", v["board"]["main_made_at"], v["board"]["present"]);
            }
            if args.iter().any(|a| a == "--json") {
                println!("{}", serde_json::to_string_pretty(&ring).unwrap_or_default());
            } else {
                print_copies(&ring, if deck { v["board"]["deck_seen"].as_u64().unwrap_or(0) } else { v["board"]["copies_seen"].as_u64().unwrap_or(0) });
            }
            Ok(())
        }
        Some("slices") => {
            if args.get(1).map(|a| a.as_str()) == Some("match") {
                send(&Request::Board {
                    art: None,
                    keep: None,
                    state: None,
                    find: None,
                    fx: None,
                    on: None,
                    ms: None,
                    verify: None,
                    arts: None,
                    siblings: None,
                    state_for: None,
                    composite_common: None,
                    target: None,
                    uploads: None,
                    finals: None,
                    copies: None,
                    peek: None,
                    force: None,
                    owners: None,
                    slices: Some("match".into()),
                    slices_state: None,
                    hunt: None,
                    owners_of: None,
                    worldcopy: None,
                    worldcopy_hashes: None,
                    hunt_abort: None,
                })?;
                let v = wait_slice_job()?;
                if args.iter().any(|a| a == "--json") {
                    println!("{}", serde_json::to_string_pretty(&v["board"]["slices_result"]).unwrap_or_default());
                } else {
                    print_slice_result(&v["board"]["slices_result"]);
                    print_matches(&v["board"]["slice_matches"]);
                }
                return Ok(());
            }
            let v = send(&Request::board())?;
            let r = &v["board"]["slices"];
            if args.iter().any(|a| a == "--json") {
                println!("{}", serde_json::to_string_pretty(r).unwrap_or_default());
                return Ok(());
            }
            let arrays = r["arrays"].as_array().cloned().unwrap_or_default();
            println!(
                "slices of tile arrays written since the main was last made (present {}, now {}): {} array(s)",
                r["since_present"],
                r["present"],
                arrays.len()
            );
            for a in &arrays {
                let slices = a["slices"].as_array().cloned().unwrap_or_default();
                println!(
                    "  array {} (DXGI format {}, {} slices, {} mip(s), state {}): {} slice(s) written: {}",
                    a["resource"].as_str().unwrap_or("?"),
                    a["dxgi_format"],
                    a["array"],
                    a["mips"],
                    a["state"].as_str().unwrap_or("unknown"),
                    slices.len(),
                    slices.iter().map(|s| format!("{}x{} ({} ago)", s["slice"], s["writes"], s["presents_ago"])).collect::<Vec<_>>().join(", ")
                );
            }
            if arrays.is_empty() {
                println!("  (none: re-equip The Caveman with the board live, then look again; board copies --deck shows the raw copies)");
            }
            Ok(())
        }
        Some("tiles") => {
            if args.get(1).map(|a| a.as_str()) == Some("clear") {
                send(&Request::Board {
                    art: None,
                    keep: None,
                    state: None,
                    find: None,
                    fx: None,
                    on: None,
                    ms: None,
                    verify: None,
                    arts: None,
                    siblings: None,
                    state_for: None,
                    composite_common: None,
                    target: None,
                    uploads: None,
                    finals: None,
                    copies: None,
                    peek: None,
                    force: None,
                    owners: None,
                    slices: Some("clear".into()),
                    slices_state: None,
                    hunt: None,
                    owners_of: None,
                    worldcopy: None,
                    worldcopy_hashes: None,
                    hunt_abort: None,
                })?;
                println!("tiles: the matched slices forgotten");
                return Ok(());
            }
            let state = arg_value(args, "--state");
            send(&Request::Board {
                art: None,
                keep: None,
                state: None,
                find: None,
                fx: None,
                on: None,
                ms: None,
                verify: None,
                arts: None,
                siblings: None,
                state_for: None,
                composite_common: None,
                target: None,
                uploads: None,
                finals: None,
                copies: None,
                peek: None,
                force: None,
                owners: None,
                slices: Some("write".into()),
                slices_state: state,
                hunt: None,
                owners_of: None,
                worldcopy: None,
                worldcopy_hashes: None,
                hunt_abort: None,
            })?;
            let v = wait_slice_job()?;
            if args.iter().any(|a| a == "--json") {
                println!("{}", serde_json::to_string_pretty(&v["board"]["slices_result"]).unwrap_or_default());
            } else {
                print_slice_result(&v["board"]["slices_result"]);
                println!(
                    "  kept: {} (written again whenever the game writes one of them; rewrites so far {})",
                    v["board"]["tiles_keep"], v["board"]["tiles_rewrites"]
                );
            }
            Ok(())
        }
        Some("hunt") => {
            if args.iter().any(|a| a == "--abort") {
                send(&Request::Board {
                    art: None,
                    keep: None,
                    state: None,
                    find: None,
                    fx: None,
                    on: None,
                    ms: None,
                    verify: None,
                    arts: None,
                    siblings: None,
                    state_for: None,
                    composite_common: None,
                    target: None,
                    uploads: None,
                    finals: None,
                    copies: None,
                    peek: None,
                    force: None,
                    owners: None,
                    slices: None,
                    slices_state: None,
                    hunt: None,
                    owners_of: None,
                    worldcopy: None,
                    worldcopy_hashes: None,
                    hunt_abort: Some(true),
                })?;
                println!("hunt: aborted (the DLL's log says what was read)");
                return Ok(());
            }
            let c = args
                .get(1)
                .filter(|a| !a.starts_with("--"))
                .ok_or("board hunt RRGGBB [--tolerance N] [--all] [--owners] [--census] [--include-targets] | board hunt --abort")?;
            let rgb = effects::hex_colour(c.trim_start_matches('#')).ok_or_else(|| format!("hunt {c}: RRGGBB"))?;
            let tolerance: Option<u8> = arg_value(args, "--tolerance").map(|t| t.parse().map_err(|_| "--tolerance N: 0 to 255")).transpose()?;
            let everything = args.iter().any(|a| a == "--everything");
            let census = args.iter().any(|a| a == "--census");
            let targets = args.iter().any(|a| a == "--include-targets");
            let assume = arg_value(args, "--assume");
            send(&Request::Board {
                art: None,
                keep: None,
                state: None,
                find: None,
                fx: None,
                on: None,
                ms: None,
                verify: None,
                arts: None,
                siblings: None,
                state_for: None,
                composite_common: None,
                target: None,
                uploads: None,
                finals: None,
                copies: None,
                peek: None,
                force: None,
                owners: None,
                slices: None,
                slices_state: None,
                hunt: Some(proto::HuntReq { rgb, tolerance, everything, assume, census, targets }),
                owners_of: None,
                worldcopy: None,
                worldcopy_hashes: None,
                hunt_abort: None,
            })?;
            let t = Instant::now();
            let limit = if census { 300 } else { 120 };
            let mut v = loop {
                std::thread::sleep(Duration::from_millis(300));
                let v = send(&Request::board())?;
                let h = &v["board"]["hunt"];
                if h["done"] == true {
                    break v;
                }
                if h.is_null() && census {
                    // the census scan runs first (the hunt exists once it is done)
                    if t.elapsed() > Duration::from_secs(limit) {
                        return Err("the census scan did not finish within the time (the DLL's log says more)".into());
                    }
                    continue;
                }
                if t.elapsed() > Duration::from_secs(limit) {
                    return Err(format!("the hunt did not finish within 120 s ({} of {} items; stage {}, {} ms since it moved, fence {}, lock misses {}; board hunt --abort clears it)", h["progress"], h["items"], h["stage"], h["since_progress_ms"], h["fence_value"], h["lock_misses"]));
                }
            };
            if args.iter().any(|a| a == "--owners") && !v["board"]["hunt"]["hits"].as_array().is_some_and(|h| h.is_empty()) {
                send(&Request::Board {
                    art: None,
                    keep: None,
                    state: None,
                    find: None,
                    fx: None,
                    on: None,
                    ms: None,
                    verify: None,
                    arts: None,
                    siblings: None,
                    state_for: None,
                    composite_common: None,
                    target: None,
                    uploads: None,
                    finals: None,
                    copies: None,
                    peek: None,
                    force: None,
                    owners: Some(true),
                    slices: None,
                    slices_state: None,
                    hunt: None,
                    owners_of: Some("hunt".into()),
                    worldcopy: None,
                    worldcopy_hashes: None,
                    hunt_abort: None,
                })?;
                let t = Instant::now();
                v = loop {
                    std::thread::sleep(Duration::from_millis(500));
                    let v = send(&Request::board())?;
                    if v["board"]["owners_pending"] == false || t.elapsed() > Duration::from_secs(60) {
                        break v;
                    }
                };
            }
            let h = &v["board"]["hunt"];
            if args.iter().any(|a| a == "--json") {
                println!("{}", serde_json::to_string_pretty(h).unwrap_or_default());
                return Ok(());
            }
            let hits = h["hits"].as_array().cloned().unwrap_or_default();
            if let Some(stuck) = h["stuck"].as_str() {
                println!("hunt: {stuck}");
            }
            if h["census"] == true {
                println!("hunt over the census (every runtime texture object, owners checked before each read); liveness: {} resource(s) watched, {} known dead ({} destruction callbacks, {} registrations failed)", h["liveness"]["watched"], h["liveness"]["dead"], h["liveness"]["callbacks"], h["liveness"]["failed"]);
            }
            println!(
                "hunt for #{} (sRGB bytes {:?}, linearised {:?}) within {} per channel: {} item(s) ({} read, {} of them under the assumed state {}, {} with an unknown state not read) in {} ms: {} hit(s)",
                c.trim_start_matches('#').to_ascii_lowercase(),
                h["rgb"],
                h["linear"],
                h["tolerance"].as_u64().map(|t| t.to_string()).unwrap_or_else(|| "24 (32 for BC1/BC3)".into()),
                h["items"],
                h["read"],
                h["assumed_read"],
                h["assume"].as_str().unwrap_or("(none)"),
                h["unknown_state"],
                h["ms"],
                hits.len()
            );
            let line = |it: &Value| {
                format!(
                    "{}{}: {}x{} DXGI {} {} mip(s), array {}, flags {:#x}; blocks at {} hit {} as {}; mean #{:02x}{:02x}{:02x}; state {}; described {} presents ago{}{}{}",
                    it["resource"].as_str().unwrap_or("?"),
                    if it["array"].as_u64().unwrap_or(1) > 1 { format!("[{}]", it["slice"]) } else { String::new() },
                    it["width"],
                    it["height"],
                    it["dxgi_format"],
                    it["mips"],
                    it["array"],
                    it["flags"].as_i64().unwrap_or(0),
                    it["blocks_at"].as_array().map(|a| a.iter().map(|p| format!("({}, {})", p[0], p[1])).collect::<Vec<_>>().join(" ")).unwrap_or_default(),
                    it["hits"].as_array().map(|a| a.iter().map(|x| if *x == true { "yes" } else { "no" }).collect::<Vec<_>>().join("/")).unwrap_or_default(),
                    it["encodings"].as_array().filter(|a| !a.is_empty()).map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("+")).unwrap_or_else(|| "none".into()),
                    it["mean"][0].as_u64().unwrap_or(0),
                    it["mean"][1].as_u64().unwrap_or(0),
                    it["mean"][2].as_u64().unwrap_or(0),
                    it["state"].as_str().map(|st| if it["assumed"] == true { format!("{st} (assumed)") } else { st.to_string() }).unwrap_or_else(|| "unknown".into()),
                    it["described_presents_ago"],
                    it["target"].as_str().map(|k| format!("; a target ({k})")).unwrap_or_default(),
                    if it["deck_shaped"] == true { "; deck-shaped" } else { "" },
                    owner_text(&it["owner"])
                )
            };
            for it in &hits {
                println!("  HIT {}", line(it));
            }
            let maybe = h["maybe"].as_array().cloned().unwrap_or_default();
            if !maybe.is_empty() {
                println!("  and {} texture(s) hit only as the colour premultiplied by a texel's alpha (a weaker sign):", maybe.len());
                for it in &maybe {
                    println!("  maybe {}", line(it));
                }
            }
            if args.iter().any(|a| a == "--all") {
                let misses = h["deck_misses"].as_array().cloned().unwrap_or_default();
                println!("  deck-shaped misses: {}", misses.len());
                for it in &misses {
                    println!("  miss {}", line(it));
                }
            }
            if hits.is_empty() {
                println!("  (no texture holds the colour at its three blocks: was the re-equip a real slot change? board uploads --deck shows what was written since; --everything reads every texture described)");
            }
            Ok(())
        }
        Some("peek") => {
            let resource = arg_value(args, "--resource").ok_or("board peek --resource 0x... --at X,Y [--mip M] [--n 4]")?;
            let at = arg_value(args, "--at").ok_or("board peek: --at X,Y")?;
            let (x, y) = at.split_once(',').ok_or("board peek: --at X,Y")?;
            let (x, y): (u32, u32) = (x.trim().parse().map_err(|_| "board peek: --at X,Y")?, y.trim().parse().map_err(|_| "board peek: --at X,Y")?);
            let mip: u32 = arg_value(args, "--mip").map(|m| m.parse().map_err(|_| "board peek: --mip M")).transpose()?.unwrap_or(0);
            let n: u32 = arg_value(args, "--n").map(|m| m.parse().map_err(|_| "board peek: --n N")).transpose()?.unwrap_or(4);
            send(&Request::Board {
                art: None,
                keep: None,
                state: None,
                find: None,
                fx: None,
                on: None,
                ms: None,
                verify: None,
                arts: None,
                siblings: None,
                state_for: None,
                composite_common: None,
                target: None,
                uploads: None,
                finals: None,
                copies: None,
                peek: Some(proto::PeekReq { resource, x, y, mip, n }),
                force: None,
                owners: None,
                slices: None,
                slices_state: None,
                hunt: None,
                owners_of: None,
                worldcopy: None,
                worldcopy_hashes: None,
                hunt_abort: None,
            })?;
            let t = Instant::now();
            let v = loop {
                std::thread::sleep(Duration::from_millis(200));
                let v = send(&Request::board())?;
                if v["board"]["peek"]["done"] == true || t.elapsed() > Duration::from_secs(15) {
                    break v;
                }
            };
            let r = &v["board"]["peek"]["result"];
            if args.iter().any(|a| a == "--json") {
                println!("{}", serde_json::to_string_pretty(&v["board"]["peek"]).unwrap_or_default());
            } else if let Some(e) = r["error"].as_str() {
                return Err(format!("peek: {e}"));
            } else if r.is_null() {
                return Err("peek: no result within 15 s (is the game presenting frames?)".into());
            } else {
                println!(
                    "peek of {} mip {} at ({}, {}), {}x{} texels, DXGI format {} ({}), read in state {} ({}): {} per {}",
                    r["resource"].as_str().unwrap_or("?"),
                    r["mip"],
                    r["left"],
                    r["top"],
                    r["width"],
                    r["height"],
                    r["dxgi_format"],
                    r["family"].as_str().unwrap_or("?"),
                    r["state"].as_str().unwrap_or("?"),
                    r["source"].as_str().unwrap_or("?"),
                    match r["family"].as_str() {
                        Some("bc7") | Some("bc3") => "16 bytes",
                        Some("bc1") => "8 bytes",
                        _ => "the bytes as stored (RGBA8: r g b a; BGRA8: b g r a)",
                    },
                    if matches!(r["family"].as_str(), Some("bc7") | Some("bc1") | Some("bc3")) { "4 x 4 block" } else { "texel" }
                );
                for row in r["texels"].as_array().into_iter().flatten() {
                    println!("  {}", row.as_array().map(|r| r.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" ")).unwrap_or_default());
                }
                if let Some(decoded) = r["decoded"].as_array().filter(|d| !d.is_empty()) {
                    println!("  decoded (r g b a a texel, {} rows):", decoded.len());
                    for row in decoded {
                        println!("  {}", row.as_array().map(|r| r.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" ")).unwrap_or_default());
                    }
                }
            }
            Ok(())
        }
        Some("uploads") => {
            let owners = args.iter().any(|a| a == "--owners");
            send(&Request::Board {
                art: None,
                keep: None,
                state: None,
                find: None,
                fx: None,
                on: None,
                ms: None,
                verify: None,
                arts: None,
                siblings: None,
                state_for: None,
                composite_common: None,
                target: None,
                uploads: Some(true),
                finals: None,
                copies: None,
                peek: None,
                force: None,
                owners: owners.then_some(true),
                slices: None,
                slices_state: None,
                hunt: None,
                owners_of: None,
                worldcopy: None,
                worldcopy_hashes: None,
                hunt_abort: None,
            })?;
            // the render thread describes the table's resources at the next frame; the owners (--owners: a scan of
            // the game's memory for every entry's pointer) take a few seconds
            let t = Instant::now();
            let v = loop {
                std::thread::sleep(Duration::from_millis(if owners { 500 } else { 400 }));
                let v = send(&Request::board())?;
                if !owners || v["board"]["owners_pending"] == false || t.elapsed() > Duration::from_secs(60) {
                    break v;
                }
            };
            // --deck: the deck-shaped subset (2D of the main's size, kept for minutes), else the main table
            let deck = args.iter().any(|a| a == "--deck");
            let mut ring = if deck { v["board"]["uploads_deck"].clone() } else { v["board"]["uploads_ring"].clone() };
            if deck {
                let (w, h) = main_size(&v);
                println!(
                    "the deck-shaped subset ({w} x {h} 2D, or not described yet when written; the main was last made at present {}, now {}):",
                    v["board"]["main_made_at"], v["board"]["present"]
                );
            }
            if false {
                let (w, h) = main_size(&v);
                let kept: Vec<Value> = ring
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|e| e["desc"]["dimension"] == 3 && e["desc"]["width"] == w && e["desc"]["height"] == h && e["desc"]["array"] == 1)
                    .cloned()
                    .collect();
                println!("deck-shaped ({w} x {h}, 2D) resources the game ended a write on: {}", kept.len());
                ring = Value::Array(kept);
            }
            if args.iter().any(|a| a == "--json") {
                println!("{}", serde_json::to_string_pretty(&ring).unwrap_or_default());
            } else {
                print_uploads(&ring, args.iter().any(|a| a == "--all"));
            }
            Ok(())
        }
        Some("find") => {
            send(&req(None, None, None, true, None, None, None))?;
            let t = Instant::now();
            loop {
                std::thread::sleep(Duration::from_millis(300));
                let v = send(&Request::board())?;
                if v["board"]["finding"] != true || t.elapsed() > Duration::from_secs(30) {
                    print_board(&v);
                    return Ok(());
                }
            }
        }
        Some("verify") => {
            let v = send(&Request::Board {
                art: None,
                keep: None,
                state: None,
                find: None,
                fx: None,
                on: None,
                ms: None,
                verify: Some(true),
                arts: None,
                siblings: None,
                state_for: None,
                composite_common: None,
                target: None,
                uploads: None,
                finals: None,
                copies: None,
                peek: None,
                force: None,
                owners: None,
                slices: None,
                slices_state: None,
                hunt: None,
                owners_of: None,
                worldcopy: None,
                worldcopy_hashes: None,
                hunt_abort: None,
            })?;
            // the read-back takes a frame or two: ask again
            std::thread::sleep(Duration::from_millis(400));
            let v = send(&Request::board()).unwrap_or(v);
            print_verify(&v["board"]["verify"]);
            Ok(())
        }
        Some("probe") => probe(args),
        Some("finish") => finish(args),
        Some("layer") => {
            // `board layer osk|ny|rgb FILE.png`: a picture as one of The Caveman's layers as it is (512 x 2048; another
            // size is resized), for putting a stock deck's own mask or normal map on the board
            let slot = args.get(1).filter(|a| matches!(a.as_str(), "osk" | "ny" | "rgb")).ok_or("board layer osk|ny|rgb FILE.png [--no-keep]")?;
            let file = args.get(2).filter(|a| !a.starts_with("--")).ok_or("board layer osk|ny|rgb FILE.png [--no-keep]")?;
            let mut img = Image::read_png(Path::new(file)).map_err(|e| format!("{file}: {e}"))?;
            if (img.width, img.height) != (dll_board::WIDTH as usize, dll_board::HEIGHT as usize) {
                img = img.resize_lanczos(dll_board::WIDTH as usize, dll_board::HEIGHT as usize);
            }
            let label = format!("layer {slot} {}", Path::new(file).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            let bc7 = build::mip_chain(&img.to_rgb(), dll_board::LEVELS, slot == "ny", caveman_core::bc7::encode);
            // the RGBA8 chain is for the in-world board's copy: re-encoded as the game's compositor would
            let arts = vec![
                BoardArt::new(&label, &bc7).for_slot(slot),
                BoardArt::with_format(&label, "rgba8", dll_board::LEVELS as u32, &rgba_chain(&effects::ridden_layer(slot, &img))).for_slot(slot),
            ];
            let keep = !args.iter().any(|a| a == "--no-keep");
            let mut r = req(None, Some(keep), None, false, None, None, None);
            if let Request::Board { arts: a, .. } = &mut r {
                *a = Some(arts);
            }
            println!("{label}: {}x{} as the {slot} layer (a BC7 chain of 12 levels and an RGBA8 chain)", img.width, img.height);
            let v = send(&r)?;
            wait_upload(&v, &sha(&bc7))
        }
        Some(other) => Err(format!("board {other}?\n{USAGE}")),
    }
}

/// `board finish KIND [--spot] [--emboss N] [--art FILE] [--game DIR] [--no-keep]`: the deck's finish, live. The
/// mask, the normal map and the holo map are built exactly as the build builds them for a deck with that finish
/// (caveman-core's effects, from the game's default layers; `--spot` puts the finish on the light parts of the art
/// only, `--emboss N` raises the print: both need the art, `--art FILE`), and sent as the arts of the osk, ny and rgb
/// slots: The Caveman's own layer textures (it is built on the holo host since 2026-10-22). The holo pattern is the
/// host's, Pearl, whatever the mode asked for: it is the preset's parameter, not a texture.
fn finish(args: &[String]) -> Result<()> {
    let usage = "board finish gloss|matte|foil|chrome|glitter|holo[-cellular|-frost|-vertical|-glitter]|none [--spot] [--emboss N] [--strength 0..1] [--art FILE] [--game DIR] [--no-keep]";
    let kind = args.get(1).filter(|a| !a.starts_with("--")).ok_or(usage)?;
    let finish: Option<effects::Finish> = match kind.as_str() {
        "none" | "plain" | "default" => None,
        k => Some(effects::Finish::from_suffix(k).ok_or_else(|| format!("finish {k}?\n{usage}"))?),
    };
    let spot = args.iter().any(|a| a == "--spot");
    let emboss: Option<f64> =
        arg_value(args, "--emboss").map(|v| v.parse::<f64>().map_err(|_| "--emboss N: a strength (the build's default is 4)".to_string())).transpose()?;
    let game = game_dir(args);
    let t = Instant::now();
    let g = Game::open(&game).map_err(|e| format!("the game at {} (for its default mask and normal map): {e}", game.display()))?;
    let (osk_default, ny_default) = build::default_layers(&g).map_err(|e| e.to_string())?;
    // the art, for the spot weights and the raised print
    let art: Image = match arg_value(args, "--art") {
        Some(f) => fit(Path::new(&f), colour_arg(args)?, &game)?.0,
        None if spot || emboss.is_some() => return Err("--spot and --emboss need the picture on the board: --art FILE.png".into()),
        None => Image::filled(512, 2048, &[255, 255, 255]),
    };
    let weight: Option<Vec<f64>> = spot.then(|| effects::spot(&art));
    let w = weight.as_deref();
    let mut osk = match finish {
        Some(f) => effects::mask(f, &osk_default, w),
        None => osk_default.clone(),
    };
    // a live holo mask carries the compositor's metal floor (the world shader multiplies the iridescence by the
    // mask's metal: B 0 in the mask copy switched the holo off on the ridden board, 25 brought it back)
    if matches!(finish, Some(effects::Finish::Holo(_))) {
        let ch = osk.channels;
        for px in osk.data.chunks_mut(ch) {
            px[2] = px[2].max(effects::HOLO_METAL_FLOOR);
        }
    }
    let glitter = matches!(finish, Some(effects::Finish::Glitter));
    let ny = effects::normal_map(emboss, glitter, &art, &ny_default, w).unwrap_or_else(|| ny_default.clone());
    // `--strength 0..1` scales the holo map's red: the shader multiplies the iridescence by it (the amount itself is
    // the preset's, 1.0 on The Caveman since the 2026-10-22 build)
    let strength: f64 = arg_value(args, "--strength")
        .map(|v| v.parse::<f64>().ok().filter(|s| (0.0..=1.0).contains(s)).ok_or_else(|| "--strength N: 0 to 1".to_string()))
        .transpose()?
        .unwrap_or(1.0);
    // the holo map: red where the holo goes (times the strength), else black (effects::HOLO_IDLE_RED); live either way
    let rgb = match finish {
        Some(effects::Finish::Holo(_)) => {
            let mut img = effects::holo_map(w);
            for px in img.data.chunks_mut(img.channels) {
                px[0] = ((px[0] as f64 * strength).round() as u8).max(1);
            }
            img
        }
        _ => effects::holo_idle(),
    };
    let holo = matches!(finish, Some(effects::Finish::Holo(_)));
    let label = format!("finish {kind}{}{}", if spot { " spot" } else { "" }, emboss.map(|e| format!(" emboss {e}")).unwrap_or_default());
    let mut arts = Vec::new();
    let mut last_sha = String::new();
    for (slot, img, normal) in [("osk", &osk, false), ("ny", &ny, true), ("rgb", &rgb, false)] {
        let bc7 = build::mip_chain(&img.to_rgb(), dll_board::LEVELS, normal, caveman_core::bc7::encode);
        last_sha = sha(&bc7);
        arts.push(BoardArt::new(&label, &bc7).for_slot(slot));
        // the RGBA8 chain is for the in-world board's copy: the mask and the normal map re-encoded as the game's
        // compositor would (measured 2026-10-22; written raw, chrome looked flat in game and right on pause)
        arts.push(BoardArt::with_format(&label, "rgba8", dll_board::LEVELS as u32, &rgba_chain(&effects::ridden_layer(slot, img))).for_slot(slot));
    }
    println!(
        "{label}: the mask, the normal map and the holo map encoded in {} ms (3 BC7 chains of 12 levels, 3 RGBA8 chains){}",
        t.elapsed().as_millis(),
        if matches!(finish, Some(effects::Finish::Holo(m)) if m != effects::FoilMode::Pearl) {
            "; the holo pattern stays the host's (Pearl): it is the preset's parameter, not a texture"
        } else {
            ""
        }
    );
    println!(
        "  (a {} finish: every write is live, no re-equip (2026-10-22: the holo switched on at once once the mask and normal copies were written as the game encodes them); the holo's pattern, colour and amount are live material parameters: board foil, board holotint, board param iridescence)",
        if holo { "holo" } else { "metal" }
    );
    let keep = !args.iter().any(|a| a == "--no-keep");
    let mut r = req(None, Some(keep), None, false, None, None, None);
    if let Request::Board { arts: a, .. } = &mut r {
        *a = Some(arts);
    }
    let v = send(&r)?;
    wait_upload(&v, &last_sha)
}

fn colour_arg(args: &[String]) -> Result<Option<[u8; 3]>> {
    match arg_value(args, "--colour").or_else(|| arg_value(args, "--color")) {
        Some(c) => effects::hex_colour(c.trim_start_matches('#')).map(Some).ok_or_else(|| format!("--colour {c}: RRGGBB")),
        None => Ok(None),
    }
}

fn game_dir(args: &[String]) -> PathBuf {
    PathBuf::from(arg_value(args, "--game").unwrap_or_else(|| caveman_cli::DEFAULT_GAME.to_string()))
}

fn sha(chain: &[u8]) -> String {
    let mut h = caveman_fx::sha256::Sha256::new();
    h.update(chain);
    caveman_fx::sha256::hex(&h.finish())
}

/// A picture fitted on The Caveman (512 x 2048 RGB): `paint` behind transparent parts, else the game's natural wood.
pub fn fit(path: &Path, paint: Option<[u8; 3]>, game: &Path) -> Result<(Image, Vec<String>)> {
    let img = Image::read_png(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let wood = match paint {
        // not used: the colour goes behind the picture
        Some(_) => Image::filled(1, 1, &[0, 0, 0]),
        None => {
            let g = Game::open(game)
                .map_err(|e| format!("the game at {} (for the natural wood behind transparent parts; or give --colour): {e}", game.display()))?;
            build::natural_wood(&g).map_err(|e| e.to_string())?
        }
    };
    build::fit_art_on(&img, &wood, paint).map_err(|e| e.to_string())
}

/// The chains for the targets added by hand whose shape is not The Caveman's (`board target 0x...`, described by
/// GetDesc in the status): the fitted art resized to each shape (Lanczos), as a BC7 chain and an RGBA8 chain of its
/// mip count. Each shape once.
fn shaped_arts(label: &str, art: &Image, status: &Value) -> Vec<BoardArt> {
    let mut out = Vec::new();
    let mut shapes: Vec<(u32, u32, u32)> = Vec::new();
    for t in status["board"]["targets"].as_array().into_iter().flatten() {
        if t["kind"] != "manual" {
            continue;
        }
        let (Some(w), Some(h), Some(mips)) = (t["width"].as_u64(), t["height"].as_u64(), t["mips"].as_u64()) else { continue };
        let shape = (w as u32, h as u32, mips as u32);
        if (shape.0, shape.1) == (dll_board::WIDTH, dll_board::HEIGHT) || shapes.contains(&shape) || !(1..=dll_board::LEVELS as u32).contains(&shape.2) {
            continue;
        }
        shapes.push(shape);
        let img = art.resize_lanczos(shape.0 as usize, shape.1 as usize);
        let bc7 = build::mip_chain(&img.to_rgb(), shape.2 as usize, false, caveman_core::bc7::encode);
        out.push(BoardArt::with_shape(label, "bc7", shape.0, shape.1, shape.2, &bc7));
        out.push(BoardArt::with_shape(label, "rgba8", shape.0, shape.1, shape.2, &rgba_chain_levels(&img, shape.2 as usize)));
        out.push(BoardArt::with_shape(label, "bc1", shape.0, shape.1, shape.2, &build::mip_chain(&img.to_rgb(), shape.2 as usize, false, bc1::encode)));
        println!("  a {}x{} chain of {} levels for a target added by hand ({} BC7 bytes)", shape.0, shape.1, shape.2, bc7.len());
    }
    out
}

/// An image (512 x 2048, any channel count) as an RGBA8 chain of 12 levels, each the box-filtered half of the one
/// before (the bytes as they are: the game's sRGB formats take them so, as the BC7 path encodes from them).
pub fn rgba_chain(art: &Image) -> Vec<u8> {
    rgba_chain_levels(art, dll_board::LEVELS)
}

/// The same with `levels` levels, for an image of any size.
pub fn rgba_chain_levels(art: &Image, levels: usize) -> Vec<u8> {
    let (w, h) = (art.width, art.height);
    let px = |x: usize, y: usize| -> [u8; 4] {
        let at = (y * w + x) * art.channels;
        let c = &art.data[at..at + art.channels];
        match art.channels {
            4 => [c[0], c[1], c[2], c[3]],
            3 => [c[0], c[1], c[2], 255],
            2 => [c[0], c[0], c[0], c[1]],
            _ => [c[0], c[0], c[0], 255],
        }
    };
    let mut level: Vec<[u8; 4]> = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| px(x, y)).collect();
    let (mut lw, mut lh) = (w, h);
    let mut out = Vec::with_capacity(dll_board::chain_len_for(dll_board::ChainFormat::Rgba8, w as u32, h as u32, levels));
    for m in 0..levels {
        for p in &level {
            out.extend_from_slice(p);
        }
        if m + 1 == levels {
            break;
        }
        let (nw, nh) = ((lw / 2).max(1), (lh / 2).max(1));
        let mut next = Vec::with_capacity(nw * nh);
        for y in 0..nh {
            for x in 0..nw {
                let mut sum = [0u32; 4];
                let mut n = 0;
                for dy in 0..2 {
                    for dx in 0..2 {
                        let (sx, sy) = ((x * 2 + dx).min(lw - 1), (y * 2 + dy).min(lh - 1));
                        let p = level[sy * lw + sx];
                        for k in 0..4 {
                            sum[k] += p[k] as u32;
                        }
                        n += 1;
                    }
                }
                next.push(std::array::from_fn(|k| ((sum[k] + n / 2) / n) as u8));
            }
        }
        level = next;
        lw = nw;
        lh = nh;
    }
    out
}

fn put(path: &Path, paint: Option<[u8; 3]>, args: &[String]) -> Result<()> {
    let (art, notes) = fit(path, paint, &game_dir(args))?;
    let label = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
    // the PNG's own alpha, fitted as the art is (before the wood or colour goes behind it), for --alpha art|invert
    let own = Image::read_png(path)
        .ok()
        .and_then(|img| build::fit_art_on(&img, &Image::filled(1, 1, &[0, 0, 0]), Some([0, 0, 0])).ok().map(|_| img))
        .map(|img| fit_alpha(&img, art.width, art.height));
    put_image(art, &label, notes, args, own)
}

/// A picture's alpha, fitted as `fit_art_on` fits the picture (the same turn, crop and resize), as a 1-channel image.
fn fit_alpha(img: &Image, width: usize, height: usize) -> Image {
    let rgba = img.to_rgba();
    let mut img = rgba.clone();
    let (mut w, mut h) = (img.width, img.height);
    if w > h && (3.0..=5.0).contains(&(w as f64 / h as f64)) {
        img = img.rotate_cw();
        (w, h) = (img.width, img.height);
    }
    let target = width as f64 / height as f64;
    if (w as f64 / h as f64 - target).abs() / target > 0.03 {
        let scale = (width as f64 / w as f64).max(height as f64 / h as f64);
        let (cw, ch) = ((width as f64 / scale).round_ties_even() as usize, (height as f64 / scale).round_ties_even() as usize);
        let (left, top) = ((w - cw) / 2, (h - ch) / 2);
        img = img.crop(left, top, left + cw, top + ch);
    }
    let img = img.resize_lanczos(width, height);
    let mut alpha = Image::new(width, height, 1);
    for (a, px) in alpha.data.iter_mut().zip(img.data.chunks_exact(4)) {
        *a = px[3];
    }
    alpha
}

/// The RGBA8 chain's alpha (`--alpha`): a constant 0 to 255 (default 255), the picture's own (`art`), or 255 minus
/// it (`invert`); `--premultiply` scales the RGB by it. The BC7 chain is the opaque fitted art either way.
fn with_alpha(art: &Image, own: Option<&Image>, args: &[String]) -> Result<Image> {
    let rule = arg_value(args, "--alpha").unwrap_or_else(|| "255".into());
    let premultiply = args.iter().any(|a| a == "--premultiply");
    let alpha_at = |i: usize| -> Result<u8> {
        match rule.as_str() {
            "art" => own.map(|o| o.data[i]).ok_or_else(|| "--alpha art: the picture's own alpha is not at hand (board fill has none)".to_string()),
            "invert" => own.map(|o| 255 - o.data[i]).ok_or_else(|| "--alpha invert: the picture's own alpha is not at hand (board fill has none)".to_string()),
            n => n.parse::<u8>().map_err(|_| format!("--alpha {n}: 0 to 255, art or invert")),
        }
    };
    let rgb = art.to_rgb();
    let mut out = Image::new(art.width, art.height, 4);
    for i in 0..art.width * art.height {
        let a = alpha_at(i)?;
        let px = &rgb.data[i * 3..i * 3 + 3];
        let c = |v: u8| if premultiply { ((v as u32 * a as u32 + 127) / 255) as u8 } else { v };
        out.data[i * 4..i * 4 + 4].copy_from_slice(&[c(px[0]), c(px[1]), c(px[2]), a]);
    }
    Ok(out)
}

/// A fitted picture (512 x 2048) on every target: the BC7 chain (the main, the siblings, the finals), an RGBA8 chain
/// (the composite, with the alpha of `--alpha`), and chains of the shapes of the targets added by hand.
fn put_image(art: Image, label: &str, notes: Vec<String>, args: &[String], own_alpha: Option<Image>) -> Result<()> {
    let t = Instant::now();
    let rgba_art = with_alpha(&art, own_alpha.as_ref(), args)?;
    let (chain, rgba) = (fxmod::board_mips(&art), rgba_chain(&rgba_art));
    // the BC1 chain: the world copy's format (the BC1 texture the game re-fills at every equip)
    let bc1_chain = build::mip_chain(&art.to_rgb(), dll_board::LEVELS, false, bc1::encode);
    let alpha_note = format!(
        "; RGBA8 alpha {}{}",
        arg_value(args, "--alpha").unwrap_or_else(|| "255".into()),
        if args.iter().any(|a| a == "--premultiply") { ", premultiplied" } else { "" }
    );
    println!("{label}: encoded in {} ms ({} bytes, 12 BC7 levels; {} bytes, 12 RGBA8 levels for the composite{alpha_note}; {} bytes, 12 BC1 levels for the world copy){}", t.elapsed().as_millis(), chain.len(), rgba.len(), bc1_chain.len(), if notes.is_empty() { String::new() } else { format!(": {}", notes.join("; ")) });
    let keep = !args.iter().any(|a| a == "--no-keep");
    let mut arts =
        vec![BoardArt::with_format(label, "rgba8", dll_board::LEVELS as u32, &rgba), BoardArt::with_format(label, "bc1", dll_board::LEVELS as u32, &bc1_chain)];
    if let Ok(status) = send(&Request::board()) {
        arts.extend(shaped_arts(label, &rgba_art, &status));
    }
    let mut r = req(Some(BoardArt::new(label, &chain)), Some(keep), None, false, None, None, None);
    if let Request::Board { arts: a, .. } = &mut r {
        *a = Some(arts);
    }
    let v = send(&r)?;
    wait_upload(&v, &sha(&chain))
}

/// `board uploads`: the writes the game ended, one line per resource, the last written first; the resources written
/// regularly (at least every fourth present: back buffers, G-buffer targets) are counted at the end unless `all`.
/// Polls the status until the slice job is done (at most 30 s).
fn wait_slice_job() -> Result<Value> {
    let t = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let v = send(&Request::board())?;
        if v["board"]["slice_job"].is_null() {
            return Ok(v);
        }
        if t.elapsed() > Duration::from_secs(30) {
            return Err(format!("the slice job did not finish within 30 s: {}", v["board"]["slice_job"]));
        }
    }
}

fn print_slice_result(r: &Value) {
    if let Some(e) = r["error"].as_str() {
        println!("slices: {e}");
        return;
    }
    match r["kind"].as_str() {
        Some("match") => {
            println!("slices match (present {}, {} matched within a mean error of {} per channel):", r["present"], r["matched"], r["max_error"]);
            for it in r["items"].as_array().into_iter().flatten() {
                match it["best"].as_object() {
                    Some(b) => println!(
                        "  {}[{}]: {} best mip {} tile ({}, {}) error {} over {} corner(s)",
                        it["resource"].as_str().unwrap_or("?"),
                        it["slice"],
                        if it["matched"] == true { "OURS:" } else { "not ours;" },
                        b["mip"],
                        b["tx"],
                        b["ty"],
                        b["error"],
                        b["corners"]
                    ),
                    None => {
                        println!("  {}[{}]: {}", it["resource"].as_str().unwrap_or("?"), it["slice"], it["note"].as_str().unwrap_or("no position compared"))
                    }
                }
            }
        }
        Some("write") => {
            println!("tiles written (present {}): {} slice(s) read back as match:", r["present"], r["written"]);
            for it in r["items"].as_array().into_iter().flatten() {
                println!(
                    "  {}[{}] in state {}: {}",
                    it["resource"].as_str().unwrap_or("?"),
                    it["slice"],
                    it["state"].as_str().unwrap_or("?"),
                    it["verdict"].as_str().unwrap_or("?")
                );
            }
        }
        _ => println!("slices: no result yet"),
    }
}

fn print_matches(m: &Value) {
    let m = m.as_array().cloned().unwrap_or_default();
    println!("matched slices kept for board tiles: {}", m.len());
    for x in &m {
        println!(
            "  {}[{}] (DXGI {}) = mip {} tile ({}, {}), error {}{}",
            x["resource"].as_str().unwrap_or("?"),
            x["slice"],
            x["dxgi_format"],
            x["mip"],
            x["tx"],
            x["ty"],
            x["error"],
            x["verdict"].as_str().filter(|v| !v.is_empty()).map(|v| format!("; last write: {v}")).unwrap_or_default()
        );
    }
}

/// The main's size as described (else The Caveman's 512 x 2048), for `--deck`.
fn main_size(v: &Value) -> (u64, u64) {
    let main = v["board"]["targets"].as_array().and_then(|t| t.iter().find(|t| t["kind"] == "main")).cloned().unwrap_or(Value::Null);
    (main["width"].as_u64().unwrap_or(dll_board::WIDTH as u64), main["height"].as_u64().unwrap_or(dll_board::HEIGHT as u64))
}

fn desc_text(d: &Value) -> String {
    match d.as_object() {
        Some(d) => format!(
            "{}x{} {} mip(s), array {}, DXGI format {}, flags {:#x}, dimension {}",
            d["width"],
            d["height"],
            d["mips"],
            d["array"],
            d["dxgi_format"],
            d["flags"].as_i64().unwrap_or(0),
            d["dimension"]
        ),
        None => "not described".into(),
    }
}

fn owner_text(o: &Value) -> String {
    match o {
        Value::Null => String::new(),
        Value::String(s) => format!("; owner: {s} (TRANSIENT?)"),
        o => format!(
            "; owner: {} {}{}{}",
            o["kind"].as_str().unwrap_or("?"),
            o["address"].as_str().unwrap_or("?"),
            o["name_hash"].as_str().map(|h| format!(", name hash {h}")).unwrap_or_default(),
            o["bind"].as_u64().map(|b| format!(", bind flags {b:#x}")).unwrap_or_default()
        ),
    }
}

/// `board copies`: the copy tracer's ring, newest first.
fn print_copies(ring: &Value, seen: u64) {
    let entries = ring.as_array().cloned().unwrap_or_default();
    if entries.is_empty() {
        println!("copies: none traced (draw depth on? the copy hooks go into the lists' tables with the depth's; {seen} seen since the DLL came in)");
        return;
    }
    println!(
        "copies: {} traced ({seen} seen since the DLL came in), newest first (presents ago: kind on list type / queue type; source -> destination):",
        entries.len()
    );
    for c in &entries {
        println!(
            "  {:>5}: {}{} on a {} list, {} queue",
            c["presents_ago"],
            c["kind"].as_str().unwrap_or("?"),
            if c["box"] == true { " (a box)" } else { "" },
            c["list_type"].as_str().unwrap_or("?"),
            c["queue_type"].as_str().unwrap_or("?")
        );
        println!(
            "         from {} sub {}: {}",
            c["src"]["resource"].as_str().unwrap_or("?"),
            c["src"]["subresource"].as_str().unwrap_or("?"),
            desc_text(&c["src"]["desc"])
        );
        println!(
            "         into {} sub {}: {}",
            c["dst"]["resource"].as_str().unwrap_or("?"),
            c["dst"]["subresource"].as_str().unwrap_or("?"),
            desc_text(&c["dst"]["desc"])
        );
    }
}

fn print_uploads(ring: &Value, all: bool) {
    let entries = ring.as_array().cloned().unwrap_or_default();
    if entries.is_empty() {
        println!("uploads: no write the game ended has been seen (draw depth on? the barrier hooks are its)");
        return;
    }
    let regulars = entries.iter().filter(|e| e["regular"] == true).count();
    println!("uploads: {} resource(s) the game ended a write on, the last written first (presents ago (count, first seen): resource, subresource, before -> after; shape):", entries.len());
    for e in &entries {
        if !all && e["regular"] == true {
            continue;
        }
        let desc = desc_text(&e["desc"]);
        let owner = owner_text(&e["owner"]);
        println!(
            "  {:>5} ({:>4}x, first {:>5} ago, presents {}..{}){}: {} sub {} {} -> {}{}; {desc}{owner}",
            e["presents_ago"],
            e["count"],
            e["first_presents_ago"],
            e["first_present"],
            e["last_present"],
            if e["regular"] == true { " regular" } else { "" },
            e["resource"].as_str().unwrap_or("?"),
            e["subresource"].as_str().unwrap_or("?"),
            e["before"].as_str().unwrap_or("?"),
            e["after"].as_str().unwrap_or("?"),
            e["target"].as_str().map(|k| format!(" [target: {k}]")).unwrap_or_default()
        );
    }
    if !all && regulars > 0 {
        println!("  (and {regulars} resource(s) written regularly, at least every fourth present, not listed: board uploads --all)");
    }
}

/// Waits (up to 15 s) for the DLL to write the art, and says what happened.
fn wait_upload(first: &Value, digest: &str) -> Result<()> {
    let t = Instant::now();
    let mut v = first.clone();
    loop {
        let b = &v["board"];
        if b["last_upload"]["sha256"] == digest && b["pending"].is_null() {
            let u = &b["last_upload"];
            println!(
                "written into The Caveman's texture ({}): {} bytes, {} us in the frame, waited {} ms; state {}",
                u["resources"].as_array().map(|r| r.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default(),
                u["bytes"],
                u["frame_us"],
                u["waited_ms"],
                u["states"]
                    .as_array()
                    .and_then(|s| s.first())
                    .map(|s| format!("{} ({})", s["states"].as_str().unwrap_or("?"), s["source"].as_str().unwrap_or("?")))
                    .unwrap_or_default()
            );
            return Ok(());
        }
        if b["pending"].is_null() && b["last_error"].as_str().is_some_and(|e| !e.is_empty()) {
            print_board(&v);
            return Err(format!("not written: {}", b["last_error"].as_str().unwrap_or("")));
        }
        if t.elapsed() > Duration::from_secs(15) {
            print_board(&v);
            return Err(if b["found"] == true {
                "not written within 15 s (is the game presenting frames? board status)".into()
            } else {
                "The Caveman's texture is not found yet: the art waits and goes on as soon as it is (board status)".into()
            });
        }
        std::thread::sleep(Duration::from_millis(200));
        v = send(&Request::board())?;
    }
}

fn fx_cmd(args: &[String]) -> Result<()> {
    let name = args.first().map(|s| s.to_ascii_lowercase()).ok_or("board fx list | NAME on|off | NAME pulse [MS]")?;
    if name == "list" {
        let v = send(&Request::board())?;
        print_fx(&v);
        return Ok(());
    }
    let c = CAVE
        .iter()
        .find(|c| c.name == name || format!("fx.cave.{}", c.name) == name)
        .ok_or_else(|| format!("no effect {name}: {}", CAVE.iter().map(|c| c.name).collect::<Vec<_>>().join(", ")))?;
    let (on, ms) = match args.get(1).map(|s| s.to_ascii_lowercase()).as_deref() {
        Some("on") => (Some(true), None),
        Some("off") => (Some(false), None),
        Some("pulse") => {
            (None, Some(args.get(2).map(|s| s.parse::<u64>().map_err(|_| format!("{s}: milliseconds"))).transpose()?.unwrap_or(caveman_fx::rules::PULSE_MS)))
        }
        _ => return Err(format!("board fx {} on|off|pulse [MS]", c.name)),
    };
    let v = send(&req(None, None, None, false, Some(c.name.to_string()), on, ms))?;
    let what = match (on, ms) {
        (Some(true), _) => "on (until board fx ... off)".to_string(),
        (Some(false), _) => "off".to_string(),
        (_, Some(ms)) => format!("on for {ms} ms"),
        _ => String::new(),
    };
    println!("fx.cave.{}: {what} ({})", c.name, c.what);
    if v["changed"]["assets"] == 0 {
        println!(
            "note: the DLL has not found Animation/Caveman/FX.Cave.{} in the game: is the rebuilt Caveman-FX installed? (status shows the scan)",
            &c.state[5..]
        );
    }
    Ok(())
}

fn print_fx(v: &Value) {
    let live = v["fx"].as_array().cloned().unwrap_or_default();
    println!("The Caveman's effects (fx.cave.<name>; board fx NAME on|off|pulse):");
    for c in CAVE {
        let key = format!("fx.cave.{}", c.name);
        let l = live.iter().find(|f| f["name"] == key.as_str());
        let state = match l {
            Some(f) => {
                let ov = f["override_ms"].as_u64().unwrap_or(0);
                format!(
                    "{:<3} assets {} reads {}{}",
                    if f["value"] == true { "ON" } else { "off" },
                    f["assets"],
                    f["reads"],
                    if ov > 3_600_000 {
                        " (on until off)".to_string()
                    } else if ov > 0 {
                        format!(" ({:.1} s left)", ov as f64 / 1000.0)
                    } else {
                        String::new()
                    }
                )
            }
            None => "not found in the game yet".to_string(),
        };
        let plays = match c.usage {
            Usage::Continuous => "while on".to_string(),
            Usage::OneOff => format!("one-shot, every {} s while on", c.cooldown),
        };
        println!("  {:<11} {:<52} {:<28} {state}", c.name, c.what, plays);
    }
}

/// The read-back check's result (`board verify`, and after each write).
fn print_verify(v: &Value) {
    if v.is_null() {
        println!("  read-back check: none yet (board art or board deck first)");
        return;
    }
    println!("  read-back check of {:?} ({}):", v["art"].as_str().unwrap_or("?"), v["stage"].as_str().unwrap_or("?"));
    for t in v["targets"].as_array().into_iter().flatten() {
        println!(
            "    texture {}: right after the write: {}; 2 s later: {}",
            t["resource"].as_str().unwrap_or("?"),
            t["verified_now"].as_str().unwrap_or("?"),
            t["verified_later"].as_str().unwrap_or("?")
        );
    }
}

fn print_board(v: &Value) {
    let b = &v["board"];
    if b.is_null() {
        println!("{v}");
        return;
    }
    print_verify(&b["verify"]);
    if b["usable"] != true && b["not_usable"].as_str().is_some_and(|s| !s.is_empty()) {
        println!("The Caveman: not usable: {}", b["not_usable"].as_str().unwrap_or(""));
    }
    let targets = b["targets"].as_array().cloned().unwrap_or_default();
    if targets.is_empty() {
        println!(
            "The Caveman: texture not found{}{}",
            if b["finding"] == true { " (looking now)" } else { "" },
            b["last_scan"].as_str().filter(|s| !s.is_empty()).map(|s| format!("; last look: {s}")).unwrap_or_default()
        );
        if let Some(r) = b["refused"].as_str().filter(|s| !s.is_empty()) {
            println!("  refused: {r}");
        }
    }
    for t in &targets {
        println!(
            "The Caveman's texture ({}, id {}{}): asset {}, runtime texture {}, D3D12 resource {}{}{}",
            t["kind"].as_str().unwrap_or("main"),
            t["id"].as_str().unwrap_or("?"),
            t["hash"].as_str().map(|h| format!(", name hash {h}")).unwrap_or_default(),
            t["asset"].as_str().unwrap_or(if t["raw"] == true { "none (a raw resource)" } else { "?" }),
            t["texture"].as_str().unwrap_or(if t["raw"] == true { "none" } else { "?" }),
            t["resource"].as_str().unwrap_or("?"),
            if t["dxgi_format"].is_null() {
                String::new()
            } else {
                format!("; GetDesc: {}x{}, DXGI format {}, {} mips", t["width"], t["height"], t["dxgi_format"], t["mips"])
            },
            t["last_seen"].as_str().filter(|s| !s.is_empty()).map(|s| format!("; last barrier seen {s}")).unwrap_or_default()
        );
        if let Some(r) = t["refused"].as_str().filter(|s| !s.is_empty()) {
            println!("  not written: {r}");
        }
        if let Some(n) = t["note"].as_str().filter(|s| !s.is_empty()) {
            println!("  note: {n}");
        }
        if let Some(m) = t["manual_state"].as_str() {
            println!("  state set by hand for it: {m}");
        }
        let s = &t["state"];
        match s["ok"] == true {
            true => println!("  state: {} ({})", s["states"].as_str().unwrap_or("?"), s["source"].as_str().unwrap_or("?")),
            false => println!("  state: UNKNOWN: {}", s["error"].as_str().unwrap_or("?")),
        }
        println!(
            "  RenderCore2 object {} (bind flags {}, +0xf8 {}); game barriers seen on it: {} {}",
            t["rc2"].as_str().unwrap_or(if t["raw"] == true { "none" } else { "?" }),
            t["rc2_bind"],
            t["rc2_state"],
            t["seen"]["transitions"],
            t["seen"]["states"].as_array().map(|a| a.iter().map(|x| x.as_str().unwrap_or("-").to_string()).collect::<Vec<_>>().join(" ")).unwrap_or_default()
        );
        match t["art"].as_object() {
            Some(a) => println!(
                "  art on it: {}... ({:.1} s ago)",
                &a["sha256"].as_str().unwrap_or("")[..12.min(a["sha256"].as_str().unwrap_or("").len())],
                a["ms_ago"].as_u64().unwrap_or(0) as f64 / 1000.0
            ),
            None => println!("  art on it: none from the DLL (its own: plain white)"),
        }
    }
    println!("  equipped: {}", b["equipped"].as_str().unwrap_or("?"));
    if let Some(a) = b["art"].as_object() {
        println!(
            "  art: {} ({}...){}",
            a["label"].as_str().unwrap_or(""),
            &a["sha256"].as_str().unwrap_or("")[..12.min(a["sha256"].as_str().unwrap_or("").len())],
            if a["keep"] == true { ", kept (put back on a texture made again)" } else { "" }
        );
    }
    if let Some(p) = b["pending"].as_object() {
        println!("  waiting to write: {} ({} ms)", p["label"].as_str().unwrap_or(""), p["waiting_ms"]);
    }
    if let Some(m) = b["manual_state"].as_str() {
        println!("  state set by hand: {m}");
    }
    println!(
        "  uploads: {}; the game's barriers on it seen: {} (watching: {})",
        b["uploads"],
        b["barriers_seen"],
        if b["watching"] == true { "yes" } else { "NO: draw depth is off or not hooked" }
    );
    if let Some(e) = b["last_error"].as_str().filter(|s| !s.is_empty()) {
        println!("  last error: {e}");
    }
    for l in b["log"].as_array().into_iter().flatten().rev().take(5).collect::<Vec<_>>().into_iter().rev() {
        println!("  | {}", l.as_str().unwrap_or(""));
    }
    if v["fx"].is_array() {
        print_fx(v);
    }
}

/// A deck PNG of the library: a path, or part of a file name under `<game>\Caveman\Decks` (and `\Imports`; also the
/// folders from before 2026-10-30, `Caveman Decks` and `Caveman Imports` in the game folder, while they are there:
/// the trainer reads and never moves them).
pub fn find_deck(game: &Path, query: &str) -> Result<PathBuf> {
    let p = PathBuf::from(query);
    if p.is_file() {
        return Ok(p);
    }
    let roots = [caveman_cli::folders::decks(game), caveman_cli::folders::imports(game), game.join("Caveman Decks"), game.join("Caveman Imports")];
    let mut found = Vec::new();
    for r in &roots {
        if r.is_dir() {
            collect_pngs(r, &mut found);
        }
    }
    let q = query.to_ascii_lowercase();
    let stem = |p: &PathBuf| p.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    // the whole file name too ("Dan Da Dan.png")
    let name = |p: &PathBuf| p.file_name().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let exact: Vec<&PathBuf> = found.iter().filter(|p| name(p) == q || stem(p) == q || stem(p).split('.').next() == Some(q.as_str())).collect();
    if exact.len() == 1 {
        return Ok(exact[0].clone());
    }
    let hits: Vec<&PathBuf> = found.iter().filter(|p| stem(p).contains(&q)).collect();
    match hits.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(format!("no deck PNG named like {query:?} in {}", roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join(" or "))),
        many => Err(format!(
            "{} decks match {query:?}: {}{}",
            many.len(),
            many.iter().take(20).map(|p| p.display().to_string()).collect::<Vec<_>>().join("; "),
            if many.len() > 20 { "; ..." } else { "" }
        )),
    }
}

fn collect_pngs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            // Caveman's own previews are not decks
            if !p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("previews")) {
                collect_pngs(&p, out);
            }
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("png")) {
            out.push(p);
        }
    }
}

/// Read only, no injection: The Caveman's texture found from outside (ReadProcessMemory), as the DLL finds it.
fn probe(args: &[String]) -> Result<()> {
    let game = find_game(arg_value(args, "--pid").and_then(|p| p.parse().ok()))?;
    let process = Handle(unsafe { win::OpenProcess(win::PROCESS_QUERY_INFORMATION | win::PROCESS_VM_READ, 0, game.pid) });
    if !process.valid() {
        return Err(format!("cannot open Skate.exe (pid {}) for reading: error {}", game.pid, unsafe { win::GetLastError() }));
    }
    check_build(process.0)?;
    let base = modules(game.pid).into_iter().find(|(n, _, _)| n.eq_ignore_ascii_case("skate.exe")).map(|(_, b, _)| b).ok_or("cannot find Skate.exe's base")?;
    win::set_target(process.0, game.pid);
    let ids = dll_board::Ids::for_game(base)?;
    println!("Skate.exe pid {} at {base:#x}: the board's pins hold (TextureAsset vtable, type, runtime texture and RenderCore2 vtables)", game.pid);
    let t = Instant::now();
    let mut s = caveman_fx::scan::WordScan::new(&[ids.asset_vtable as u64], 1 << 20)?;
    while !s.step(256 << 20, 1000) {}
    println!("{} TextureAsset objects in {} MB ({:.1} s)", s.found.len(), s.bytes >> 20, t.elapsed().as_secs_f64());
    let mut any = false;
    for &a in &s.found {
        match dll_board::read_chain(a, &ids) {
            Ok(c) => {
                any = true;
                println!(
                    "The Caveman: asset {a:#x} -> runtime texture {:#x} -> D3D12 resource {:#x} (vtable {:#x})",
                    c.texture,
                    c.resource,
                    win::read_ptr(c.resource).unwrap_or(0)
                );
                println!("  RenderCore2 object {:#x}: bind flags {}, +0xf8 {}", c.rc2, c.rc2_bind, c.rc2_state);
                let mut raw = vec![0u8; 0x120];
                if win::read_into(c.texture, &mut raw) {
                    println!("  runtime texture +0x00..+0x120:");
                    for (k, row) in raw.chunks(16).enumerate() {
                        println!("    +{:03x}  {}", k * 16, row.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "));
                    }
                }
            }
            Err(e) if e.starts_with("not a TextureAsset") || e == "another texture" => {}
            Err(e) => println!("refused {a:#x}: {e}"),
        }
    }
    if !any {
        println!("The Caveman's texture is not loaded (is the rebuilt Caveman-FX installed? equip The Caveman once)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trainer, the mod and the DLL agree: the texture's name, the chain's size, the effects' fx states.
    #[test]
    fn the_three_agree() {
        assert_eq!(fxmod::cave_texture_name("000 - "), dll_board::TEXTURE_NAME);
        assert_eq!(fxmod::cave_mips().len(), dll_board::chain_len());
        for c in CAVE {
            let state = caveman_fx::rules::fx_key(&fxmod::state_name(c.state));
            assert_eq!(state, format!("fx.cave.{}", c.name));
            assert_eq!(caveman_fx::worker::cave_fx_key(c.name), state);
            assert_eq!(caveman_fx::worker::cave_fx_key(&state), state);
        }
        // the art round trip the pipe makes
        let chain = fxmod::cave_mips();
        let art = BoardArt::new("white", &chain);
        assert_eq!(art.chain().unwrap(), chain);
        assert_eq!(art.sha256, sha(&chain));
    }

    #[test]
    fn finds_library_decks() {
        let dir = std::env::temp_dir().join(format!("caveman-board-decks-{}", std::process::id()));
        let decks = caveman_cli::folders::decks(&dir);
        let imports = caveman_cli::folders::imports(&dir);
        std::fs::create_dir_all(decks.join("000").join("previews")).unwrap();
        std::fs::create_dir_all(imports.join("pack")).unwrap();
        for f in ["000/Cool Deck.ff0000.png", "000/Cooler Deck.png", "000/previews/Cool Deck.png", "Other.holo-frost.png"] {
            std::fs::write(decks.join(f), b"x").unwrap();
        }
        std::fs::write(imports.join("pack").join("Imported Thing.png"), b"x").unwrap();
        assert!(find_deck(&dir, "cool deck").unwrap().ends_with("Cool Deck.ff0000.png"), "the name before its suffixes");
        assert!(find_deck(&dir, "cooler").unwrap().ends_with("Cooler Deck.png"));
        assert!(find_deck(&dir, "cool").is_err(), "two match");
        assert!(find_deck(&dir, "other").unwrap().ends_with("Other.holo-frost.png"));
        assert!(find_deck(&dir, "imported").unwrap().ends_with("Imported Thing.png"));
        assert!(find_deck(&dir, "nothing").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
