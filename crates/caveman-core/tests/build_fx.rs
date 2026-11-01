//! Builds the Caveman-FX mod (fxmod.rs) into the session scratchpad and reads it back from its own archive: the fx
//! states (BoolAssets in the game's GUID style), each test deck's item, its effect asset firing on its fx state
//! (TriggerType 6, usage, cooldown, the state imported by GUID), the blueprints, and the master item list. Needs the
//! game (read only); run with `--ignored`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use caveman_core::build::{self, MASTER, SHARED};
use caveman_core::ebx::{self, Document, Guid, Pointer};
use caveman_core::effects::{self, Usage};
use caveman_core::fxmod::{self, FxLook, CAVE, COMBOS, DECKS, LIBRARY, SPRITE, STATES, TUNED};
use caveman_core::game::{Game, Layer, ITEMS_TOC};
use caveman_core::image::Image;
use caveman_core::{shader, shaderstate, texture, toc};

fn scratch() -> PathBuf {
    std::env::var_os("CAVEMAN_SCRATCH").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(r"C:\Users\thetr\AppData\Local\Temp\claude\C--Users-thetr-Projects-ReSkate\c6d3c964-296a-4f0b-bedc-241e3cf960f6\scratchpad")
    })
}

fn import_of(doc: &Document, data: &ebx::Data) -> (Guid, Guid) {
    match data.as_pointer() {
        Some(Pointer::External(k)) => doc.imports[k],
        other => panic!("not an import: {other:?}"),
    }
}

#[test]
#[ignore]
fn build_and_read_back() {
    let game = Arc::new(Game::open(Path::new(r"E:\Games\ReSkate")).unwrap());
    let mods = scratch().join("fxmod").join("Mods");
    std::fs::create_dir_all(&mods).unwrap();
    let prefix = "000 - ";
    let built = fxmod::build_fx_mod(game.clone(), &mods, fxmod::MOD, prefix, &mut |l| println!("{l}")).unwrap();
    assert_eq!(built.report.decks.len(), DECKS.len() + COMBOS.len() + LIBRARY.len() + 1, "the test, combo and library decks, and The Caveman");

    // the mod's shared bundle, read from its own archive
    let layer = Layer::open(&mods.join(fxmod::MOD), game.clone());
    let toc = layer.toc(ITEMS_TOC).unwrap();
    let b = toc.bundles.iter().find(|b| b.name == SHARED).expect("the shared bundle");
    let (manifest, files) = layer.bundle(b.region.as_ref().unwrap()).unwrap();
    let doc = |name: &str| -> Document {
        let i = manifest.ebx_index(name).unwrap_or_else(|| panic!("no {name} in the mod"));
        let loc = files[manifest.ebx_file(i) - 1];
        assert!(loc.patch, "{name} comes from the mod's archive");
        ebx::read_document(&layer.decode(&loc).unwrap()).unwrap()
    };

    // the fx states
    let mut state_refs: HashMap<&str, (Guid, Guid)> = HashMap::new();
    for short in STATES.iter().copied().chain(CAVE.iter().map(|c| c.state)) {
        let name = fxmod::state_name(short);
        let d = doc(&name.to_ascii_lowercase());
        assert_eq!(d.root_type, "BoolAsset");
        assert_eq!(d.instances.len(), 1);
        assert_eq!(d.file_guid, d.instances[0].instance_guid, "{name}: file GUID = instance GUID");
        assert_eq!(d.reference(), fxmod::state_ref(short));
        assert_eq!(d.get("Name", 0).unwrap().as_str().unwrap(), name);
        assert_eq!(d.get("Default", 0).unwrap().as_bool(), Some(false));
        assert!(d.imports.is_empty());
        state_refs.insert(short, d.reference());
    }

    // the decks
    let shared = game.bundle(SHARED, ITEMS_TOC).unwrap();
    let game_ref = |name: &str| -> (Guid, Guid) {
        let i = shared.manifest.ebx_index(name).unwrap();
        ebx::read_document(&game.decode(&shared.files[shared.manifest.ebx_file(i)]).unwrap()).unwrap().reference()
    };
    let master = doc(MASTER);
    for d in DECKS {
        let key = fxmod::deck_key(&format!("{prefix}{}", d.name));
        let base = build::base_name(&key);
        let item = doc(&format!("items/board_bottomart/own_{base}"));
        assert!(master.imports.contains(&item.reference()), "{}: in the master list", d.name);
        let di = item.instances_of("DelMarCustomizationItemData")[0];
        assert_eq!(item.get("PreviewVFX", di).unwrap().as_bool(), Some(true));
        let style = match d.look {
            FxLook::Aura(_) => "aura",
            FxLook::Nyan => "nyan",
            FxLook::Burst(_) => "burst",
            other => panic!("{}: a test deck with {other:?}", d.name),
        };
        let va = doc(&format!("characters/vfx/caveman/{base}_{style}"));
        assert_eq!(import_of(&item, item.get("SkaterVfx", di).unwrap()), va.reference(), "{}: the item's SkaterVfx", d.name);
        let ei = va.instances_of("SkaterVfxEffectItem")[0];
        let o = va.object(ei).unwrap();
        assert_eq!(o.get("TriggerType").unwrap().as_i64(), Some(6), "{}", d.name);
        assert_eq!(o.get("TriggerOutputUsage").unwrap().as_i64(), Some(if d.usage == Usage::OneOff { 1 } else { 0 }));
        assert_eq!(o.get("CooldownType").unwrap().as_i64(), Some(0));
        assert!((o.get("CooldownTime").unwrap().as_f64().unwrap() - d.cooldown).abs() < 1e-6);
        assert_eq!(import_of(&va, o.get("TriggerEffectsGameState.AssetRef").unwrap()), state_refs[d.state], "{}: fires on FX.{}", d.name, d.state);
        assert!(o.get("KillEffectsGameState.AssetRef").unwrap().as_pointer().is_none_or(|p| matches!(p, Pointer::Null)));
        let effects = o.get("Effects").unwrap().as_array().unwrap();
        assert_eq!(effects.len(), if matches!(d.look, FxLook::Aura(_)) { 2 } else { 1 }, "{}: attachments", d.name);
        let blueprint = match d.look {
            FxLook::Burst(bp) => game_ref(bp),
            _ => doc(&format!("characters/vfx/caveman/ebp_{base}_{style}")).reference(),
        };
        for v in effects {
            let Some(Pointer::Internal(t)) = v.data.as_pointer() else { panic!("an attachment is not internal") };
            let to = va.object(t).unwrap();
            assert_eq!(import_of(&va, to.get("Effect").unwrap()), blueprint, "{}: plays its blueprint", d.name);
            assert_eq!(to.get("AttachType").unwrap().as_i64(), Some(1), "on the board");
        }
        println!("{}{}: {style} on FX.{} ({:?}, cooldown {})", prefix, d.name, d.state, d.usage, d.cooldown);
    }

    // the combo decks: one asset, an effect item per part (in its VFX after the template's own entries), each with
    // its own state, usage, attachments, blueprint and InstanceGuidHash
    for c in COMBOS {
        let key = fxmod::deck_key(&format!("{prefix}{}", c.name));
        let base = build::base_name(&key);
        let item = doc(&format!("items/board_bottomart/own_{base}"));
        assert!(master.imports.contains(&item.reference()), "{}: in the master list", c.name);
        let di = item.instances_of("DelMarCustomizationItemData")[0];
        let va = doc(&format!("characters/vfx/caveman/{base}_combo"));
        assert_eq!(import_of(&item, item.get("SkaterVfx", di).unwrap()), va.reference(), "{}: the item's SkaterVfx", c.name);
        assert!(va.instances.iter().all(|i| i.exported), "{}: every instance exported", c.name);
        let listed: Vec<usize> = va
            .object(0)
            .unwrap()
            .get("VFX")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| match v.data.as_pointer() {
                Some(Pointer::Internal(i)) => Some(i),
                _ => None,
            })
            .collect();
        assert_eq!(listed.len(), c.parts.len(), "{}: an effect item per part", c.name);
        let mut identities = Vec::new();
        for (k, (part, &ei)) in c.parts.iter().zip(&listed).enumerate() {
            let o = va.object(ei).unwrap();
            assert_eq!(o.get("TriggerType").unwrap().as_i64(), Some(6), "{} part {k}", c.name);
            assert_eq!(o.get("TriggerOutputUsage").unwrap().as_i64(), Some(if part.usage == Usage::OneOff { 1 } else { 0 }));
            assert!((o.get("CooldownTime").unwrap().as_f64().unwrap() - part.cooldown).abs() < 1e-6);
            assert_eq!(import_of(&va, o.get("TriggerEffectsGameState.AssetRef").unwrap()), state_refs[part.state], "{} part {k}: FX.{}", c.name, part.state);
            identities.push(o.get("InstanceGuidHash").unwrap().as_i64());
            let effects = o.get("Effects").unwrap().as_array().unwrap();
            assert_eq!(effects.len(), if matches!(part.look, FxLook::Aura(_)) { 2 } else { 1 }, "{} part {k}: attachments", c.name);
            let blueprint = match part.look {
                FxLook::Burst(bp) => game_ref(bp),
                FxLook::Aura(_) => doc(&format!("characters/vfx/caveman/ebp_{base}_combo{k}_aura")).reference(),
                FxLook::Nyan => doc(&format!("characters/vfx/caveman/ebp_{base}_combo{k}_nyan")).reference(),
                other => panic!("{} part {k}: {other:?}", c.name),
            };
            for v in effects {
                let Some(Pointer::Internal(t)) = v.data.as_pointer() else { panic!("an attachment is not internal") };
                let to = va.object(t).unwrap();
                assert_eq!(import_of(&va, to.get("Effect").unwrap()), blueprint, "{} part {k}: plays its blueprint", c.name);
                assert_eq!(to.get("AttachType").unwrap().as_i64(), Some(1), "on the board");
            }
        }
        identities.sort();
        identities.dedup();
        assert_eq!(identities.len(), c.parts.len(), "{}: each effect item has its own InstanceGuidHash", c.name);
        println!("{}{}: {} effects on one board", prefix, c.name, c.parts.len());
    }

    // the mod's own blueprints: retuned copies of stock ones
    let own_doc = |short: &str| doc(&fxmod::own_blueprint_name(short));
    for t in TUNED {
        let mine = own_doc(t.name);
        assert_eq!(Some(&(t.name.to_string(), fxmod::own_blueprint_name(t.name), mine.reference())), built.own.iter().find(|o| o.0 == t.name));
        let i = shared.manifest.ebx_index(t.source).unwrap();
        let stock = ebx::read_document(&game.decode(&shared.files[shared.manifest.ebx_file(i)]).unwrap()).unwrap();
        check_tuned(&stock, &mine, &t.tuning, t.name);
    }

    // the library decks: every one fires on fx.test
    let oni = {
        let i = shared.manifest.ebx_index(effects::BLUEPRINT).unwrap();
        ebx::read_document(&game.decode(&shared.files[shared.manifest.ebx_file(i)]).unwrap()).unwrap()
    };
    for d in LIBRARY {
        let key = fxmod::deck_key(&format!("{prefix}{}", d.name));
        let base = build::base_name(&key);
        let item = doc(&format!("items/board_bottomart/own_{base}"));
        assert!(master.imports.contains(&item.reference()), "{}: in the master list", d.name);
        let di = item.instances_of("DelMarCustomizationItemData")[0];
        assert_eq!(item.get("PreviewVFX", di).unwrap().as_bool(), Some(true));
        let style = match d.look {
            FxLook::Aura(_) | FxLook::AuraTuned(..) => "aura",
            FxLook::Nyan => "nyan",
            FxLook::Burst(_) => "burst",
            FxLook::Stock(..) | FxLook::Own(..) => "fx",
        };
        let va = doc(&format!("characters/vfx/caveman/{base}_{style}"));
        assert_eq!(import_of(&item, item.get("SkaterVfx", di).unwrap()), va.reference(), "{}: the item's SkaterVfx", d.name);
        let ei = va.instances_of("SkaterVfxEffectItem")[0];
        let o = va.object(ei).unwrap();
        assert_eq!(o.get("TriggerType").unwrap().as_i64(), Some(6), "{}", d.name);
        assert_eq!(o.get("TriggerOutputUsage").unwrap().as_i64(), Some(if d.usage == Usage::OneOff { 1 } else { 0 }), "{}", d.name);
        assert!((o.get("CooldownTime").unwrap().as_f64().unwrap() - d.cooldown).abs() < 1e-6);
        assert_eq!(import_of(&va, o.get("TriggerEffectsGameState.AssetRef").unwrap()), state_refs["Test"], "{}: fires on FX.Test", d.name);
        let (blueprint, points): ((Guid, Guid), Vec<[f64; 3]>) = match d.look {
            FxLook::Stock(bp, at) => (game_ref(bp), at.points().to_vec()),
            FxLook::Own(short, at) => (own_doc(short).reference(), at.points().to_vec()),
            FxLook::AuraTuned(colour, tuning) => {
                let bp = doc(&format!("characters/vfx/caveman/ebp_{base}_aura"));
                check_aura(&bp, &oni, colour, &tuning, d.name);
                (bp.reference(), effects::POINTS.to_vec())
            }
            other => panic!("{}: a library deck with {other:?}", d.name),
        };
        let attachments = o.get("Effects").unwrap().as_array().unwrap();
        assert_eq!(attachments.len(), points.len(), "{}: an attachment per point", d.name);
        for (v, p) in attachments.iter().zip(&points) {
            let Some(Pointer::Internal(t)) = v.data.as_pointer() else { panic!("an attachment is not internal") };
            let to = va.object(t).unwrap();
            assert_eq!(import_of(&va, to.get("Effect").unwrap()), blueprint, "{}: plays its blueprint", d.name);
            assert_eq!(to.get("AttachType").unwrap().as_i64(), Some(1), "on the board");
            let off: Vec<f64> = ["x", "y", "z"].iter().map(|a| to.get(&format!("Offset.{a}")).unwrap().as_f64().unwrap()).collect();
            assert!(off.iter().zip(p).all(|(a, b)| (a - b).abs() < 1e-6), "{}: offset {off:?}, wanted {p:?}", d.name);
        }
        println!("{}{}: {:?} on FX.Test ({:?}, cooldown {}), {} attachment(s)", prefix, d.name, d.look, d.usage, d.cooldown, points.len());
    }

    the_caveman(&doc, &game_ref, &own_doc, &oni, &state_refs, &master, &layer, prefix);

    // the plain aura through the tuned path is the plain aura
    let plain = |name: &str| doc(&format!("characters/vfx/caveman/ebp_{}_aura", build::base_name(&fxmod::deck_key(&format!("{prefix}{name}")))));
    let (a, b) = (plain("FX Manual Aura"), plain("FXL Aura X1"));
    assert_eq!(aura_values(&a), aura_values(&b), "AuraTuning::STOCK writes what the plain aura writes");

    // the sprite prototype: texture, depot, material rows, blueprint
    let key = built.sprite_key.expect("built with the sprite prototype");
    let names: Vec<&str> = manifest.res.iter().map(|r| r.name.as_str()).collect();
    let depot_name = fxmod::sprite_depot_name(key);
    let ri = manifest.res.iter().position(|r| r.name == depot_name).expect("the sprite depot");
    let r = &manifest.res[ri];
    assert_eq!(r.rtype, shaderstate::DEPOT_TYPE);
    assert_eq!(r.rid & 1, 1, "an odd resource id");
    let loc = files[manifest.res_file(ri) - 1];
    let depot = shaderstate::read_depot(&layer.decode(&loc).unwrap(), &r.meta).unwrap();
    assert_eq!(depot.entries.len(), 1);
    assert_eq!(depot.entries[0].0, key);
    let tex = doc(SPRITE.texture);
    assert_eq!(tex.root_type, "TextureAsset");
    let (tf, ti) = tex.reference();
    let list = &depot.entries[0].1;
    assert!(list.windows(16).any(|w| w == tf.0) && list.windows(16).any(|w| w == ti.0), "the depot block names our texture");
    assert!(names.contains(&SPRITE.texture), "the texture's resource");
    let tri = manifest.res.iter().position(|r| r.name == SPRITE.texture).unwrap();
    assert_eq!(manifest.res[tri].rid & 1, 1);
    assert_eq!(tex.get("Resource", 0).unwrap().as_resource_ref().unwrap().id, manifest.res[tri].rid);
    let header = texture::TextureHeader::parse(&layer.decode(&files[manifest.res_file(tri) - 1]).unwrap(), &manifest.res[tri].meta).unwrap();
    assert_eq!((header.format, header.width, header.height, header.mips, header.first_mip), (67, 1024, 1024, 11, 0), "whole BC7 sRGB");
    let sb = own_doc(SPRITE.name);
    let emitters = effects::components_of(&sb, "EmitterGraphEntityData");
    assert_eq!(emitters.len(), 1);
    assert_eq!(effects::material_keys(&sb, emitters[0]), vec![key], "the star emitter draws with the new key");
    assert_eq!(effects::components_of(&sb, "LightEffectEntityData").len(), 1);
    // the rows in the mod's globals.toc: an alias of the source key in the database and the texture lookup
    let gtoc = layer.toc(shader::GLOBALS_TOC).unwrap();
    let gb = gtoc.bundles.iter().find(|b| b.name == shader::CONFIG_BUNDLE).expect("the configuration bundle in globals.toc");
    let (gm, gfiles) = layer.bundle(gb.region.as_ref().unwrap()).unwrap();
    let res_of = |name: &str| -> (Vec<u8>, [u8; 16]) {
        let i = gm.res_index(name).unwrap();
        let loc = gfiles[gm.res_file(i) - 1];
        assert!(loc.patch, "{name} comes from the mod");
        (layer.decode(&loc).unwrap(), gm.res[i].meta)
    };
    let (db, meta) = res_of(fxmod::PROGRAM_LOOKUP);
    let src = shaderstate::program_records(&db, &meta, SPRITE.source_key as u64).unwrap().unwrap();
    assert_eq!(shaderstate::program_records(&db, &meta, key as u64).unwrap(), Some(src.clone()));
    let (tl, _) = res_of(fxmod::TEXTURE_LOOKUP);
    let rows = shaderstate::texture_rows(&tl).unwrap();
    let set = |k: u64| rows.iter().find(|r| r.0 == k).map(|r| r.1);
    assert_eq!(set(key as u64), set(SPRITE.source_key as u64));
    assert!(rows.windows(2).all(|w| w[0].0 < w[1].0), "the texture lookup stays sorted");
    let config = game.bundle(shader::CONFIG_BUNDLE, shader::GLOBALS_TOC).unwrap();
    let base_db = game.decode(&config.files[config.manifest.res_file(config.manifest.res_index(fxmod::PROGRAM_LOOKUP).unwrap())]).unwrap();
    assert_eq!(shaderstate::record_count(&db).unwrap(), shaderstate::record_count(&base_db).unwrap(), "no records added");
    println!("sprite: key {key} aliases {} (records {src:?}), depot {depot_name}, texture {}", SPRITE.source_key, SPRITE.texture);

    println!(
        "verified {} fx states, {} decks, {} combo deck(s), {} library decks, The Caveman, {} blueprints of the mod's own in {}",
        STATES.len() + CAVE.len(),
        DECKS.len(),
        COMBOS.len(),
        LIBRARY.len(),
        built.own.len(),
        mods.join(fxmod::MOD).display()
    );
}

/// QualityScalable values (Ultra) of a path in an object.
fn ultra(o: &ebx::Object, path: &str) -> f64 {
    o.get(&format!("{path}.Ultra")).unwrap().as_f64().unwrap()
}

/// The parameter `pid` of an emitter, (x, y, z).
fn param(o: &ebx::Object, pid: i64) -> Option<[f64; 3]> {
    o.get("EmitterGraphParams").unwrap().as_array().unwrap().iter().find_map(|v| {
        let p = v.data.as_object()?;
        (p.get("PropertyId").ok()?.as_i64()? == pid).then(|| ["x", "y", "z"].map(|a| p.get(&format!("Value.{a}")).unwrap().as_f64().unwrap()))
    })
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-4 * a.abs().max(b.abs()).max(1.0)
}

/// A retuned copy against its stock blueprint: every live emitter's overridden counts, lives, size and colour, every
/// light's intensity, reach and life, scaled as the tuning says.
fn check_tuned(stock: &Document, mine: &Document, t: &effects::EffectTuning, name: &str) {
    let (se, me) = (effects::components_of(stock, "EmitterGraphEntityData"), effects::components_of(mine, "EmitterGraphEntityData"));
    assert_eq!(se.len(), me.len(), "{name}: the same emitters");
    let mut checked = 0;
    for (&s, &m) in se.iter().zip(&me) {
        let (so, mo) = (stock.object(s).unwrap(), mine.object(m).unwrap());
        let flag = |o: &ebx::Object, f: &str| o.get(&format!("EmitterGraphOverrides.{f}")).unwrap().as_bool().unwrap();
        for (f, field, k) in [("IsSpawnRateOverrideSet", "SpawnRate", t.count), ("IsParticleLifeSpanOverrideSet", "ParticleLifeSpan", t.life)] {
            if flag(so, f) {
                let (a, b) = (ultra(so, &format!("EmitterGraphOverrides.{field}")), ultra(mo, &format!("EmitterGraphOverrides.{field}")));
                assert!(close(a * k, b), "{name}: {field} {a} x {k} = {b}?");
                checked += 1;
            }
        }
        if let (Some(a), Some(b)) = (param(so, effects::SPRITE_SIZE), param(mo, effects::SPRITE_SIZE)) {
            assert!(close(t.scale_size(a[0]), b[0]) && close(t.scale_size(a[1]), b[1]), "{name}: size {a:?} x {} = {b:?}?", t.size);
            checked += 1;
        }
        if let (Some(a), Some(b)) = (param(so, effects::TRAIL_COLOUR), param(mo, effects::TRAIL_COLOUR)) {
            assert!((0..3).all(|c| close(a[c] * t.brightness, b[c])), "{name}: colour {a:?} x {} = {b:?}?", t.brightness);
            checked += 1;
        }
    }
    for (s, m) in effects::components_of(stock, "LightEffectEntityData").into_iter().zip(effects::components_of(mine, "LightEffectEntityData")) {
        let (a, b) = (stock.get("Lifetime", s).unwrap().as_f64().unwrap(), mine.get("Lifetime", m).unwrap().as_f64().unwrap());
        assert!(close(if a > 0.0 { a * t.light_life } else { 0.0 }, b), "{name}: light life {a} -> {b}");
        checked += 1;
    }
    assert!(checked > 0, "{name}: nothing checked");
    println!("{name}: {checked} retuned values match");
}

/// What an aura blueprint sets: the light's colour, intensity and reach, every live trail's colours, width and life.
fn aura_values(bp: &Document) -> Vec<f64> {
    let mut out = Vec::new();
    let li = bp.instances_of("PbrSphereLightEntityData")[0];
    for f in ["Color.x", "Color.y", "Color.z", "Intensity", "AttenuationRadius"] {
        out.push(bp.get(f, li).unwrap().as_f64().unwrap());
    }
    for e in effects::components_of(bp, "EmitterGraphEntityData") {
        let o = bp.object(e).unwrap();
        for pid in [effects::TRAIL_COLOUR, effects::TRAIL_COLOUR2, effects::WIDTH] {
            out.extend(param(o, pid).unwrap_or_default());
        }
        out.push(ultra(o, "EmitterGraphOverrides.ParticleLifeSpan"));
    }
    out
}

/// A tuned aura's blueprint: the light at the tuning's intensity and reach, the trails at its brightness, width and life.
fn check_aura(bp: &Document, oni: &Document, colour: [u8; 3], t: &effects::AuraTuning, name: &str) {
    let c = effects::linear(colour);
    let li = bp.instances_of("PbrSphereLightEntityData")[0];
    assert!(close(bp.get("Intensity", li).unwrap().as_f64().unwrap(), t.light_intensity), "{name}: light intensity");
    assert!(close(bp.get("AttenuationRadius", li).unwrap().as_f64().unwrap(), t.light_radius), "{name}: light reach");
    let mut trails = 0;
    let stock_widths: Vec<f64> = effects::components_of(oni, "EmitterGraphEntityData")
        .into_iter()
        .filter_map(|e| oni.object(e).ok().and_then(|o| param(o, effects::TRAIL_COLOUR).and(param(o, effects::WIDTH))))
        .map(|w| w[0])
        .collect();
    for e in effects::components_of(bp, "EmitterGraphEntityData") {
        let o = bp.object(e).unwrap();
        let Some(col) = param(o, effects::TRAIL_COLOUR) else { continue };
        assert!((0..3).all(|k| close(col[k], c[k] * t.trail_boost)), "{name}: trail colour {col:?}");
        if let Some(life) = t.trail_life {
            assert!(close(ultra(o, "EmitterGraphOverrides.ParticleLifeSpan"), life), "{name}: trail life");
        }
        let w = param(o, effects::WIDTH).unwrap()[0];
        assert!(close(w, stock_widths[trails] * t.trail_width.unwrap_or(1.0)), "{name}: trail width {w} from {}", stock_widths[trails]);
        trails += 1;
    }
    assert_eq!(trails, 2, "{name}: two trails");
}

/// The Caveman: its item, its combo asset (an effect item per part of `CAVE`, each on its own fx.cave.* state, with its
/// usage, cooldown, blueprint and an attachment per board point), and its graphic: a whole BC7 sRGB texture, 512 x 2048,
/// 12 mips, first mip 0, an odd resource id, whose chunk holds exactly `fxmod::cave_mips()` (what `board reset` uploads).
#[allow(clippy::too_many_arguments)]
fn the_caveman(
    doc: &dyn Fn(&str) -> Document,
    game_ref: &dyn Fn(&str) -> (Guid, Guid),
    own_doc: &dyn Fn(&str) -> Document,
    oni: &Document,
    state_refs: &HashMap<&str, (Guid, Guid)>,
    master: &Document,
    layer: &Layer,
    prefix: &str,
) {
    let key = fxmod::deck_key(&fxmod::cave_display(prefix));
    let base = build::base_name(&key);
    let item = doc(&format!("items/board_bottomart/own_{base}"));
    assert!(master.imports.contains(&item.reference()), "The Caveman is in the master list");
    let di = item.instances_of("DelMarCustomizationItemData")[0];
    let va = doc(&format!("characters/vfx/caveman/{base}_combo"));
    assert_eq!(import_of(&item, item.get("SkaterVfx", di).unwrap()), va.reference(), "The Caveman's SkaterVfx");
    assert!(va.instances.iter().all(|i| i.exported));
    let listed: Vec<usize> = va
        .object(0)
        .unwrap()
        .get("VFX")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| match v.data.as_pointer() {
            Some(Pointer::Internal(i)) => Some(i),
            _ => None,
        })
        .collect();
    assert_eq!(listed.len(), CAVE.len(), "an effect item per effect");
    let mut identities = Vec::new();
    for (k, (c, &ei)) in CAVE.iter().zip(&listed).enumerate() {
        let o = va.object(ei).unwrap();
        assert_eq!(o.get("TriggerType").unwrap().as_i64(), Some(6), "{}", c.name);
        assert_eq!(o.get("TriggerOutputUsage").unwrap().as_i64(), Some(if c.usage == Usage::OneOff { 1 } else { 0 }), "{}", c.name);
        assert_eq!(o.get("CooldownType").unwrap().as_i64(), Some(0));
        assert!((o.get("CooldownTime").unwrap().as_f64().unwrap() - c.cooldown).abs() < 1e-6, "{}", c.name);
        assert_eq!(import_of(&va, o.get("TriggerEffectsGameState.AssetRef").unwrap()), state_refs[c.state], "{}: fires on FX.{}", c.name, c.state);
        identities.push(o.get("InstanceGuidHash").unwrap().as_i64());
        let (blueprint, points): ((Guid, Guid), Vec<[f64; 3]>) = match c.look {
            FxLook::Stock(bp, at) => (game_ref(bp), at.points().to_vec()),
            FxLook::Own(short, at) => (own_doc(short).reference(), at.points().to_vec()),
            FxLook::AuraTuned(colour, tuning) => {
                let bp = doc(&format!("characters/vfx/caveman/ebp_{base}_combo{k}_aura"));
                check_aura(&bp, oni, colour, &tuning, c.name);
                (bp.reference(), effects::POINTS.to_vec())
            }
            FxLook::Nyan => (doc(&format!("characters/vfx/caveman/ebp_{base}_combo{k}_nyan")).reference(), effects::NYAN_POINTS.to_vec()),
            other => panic!("{}: {other:?}", c.name),
        };
        let attachments = o.get("Effects").unwrap().as_array().unwrap();
        assert_eq!(attachments.len(), points.len(), "{}: an attachment per point", c.name);
        for (v, p) in attachments.iter().zip(&points) {
            let Some(Pointer::Internal(t)) = v.data.as_pointer() else { panic!("an attachment is not internal") };
            let to = va.object(t).unwrap();
            assert_eq!(import_of(&va, to.get("Effect").unwrap()), blueprint, "{}: plays its blueprint", c.name);
            assert_eq!(to.get("AttachType").unwrap().as_i64(), Some(1), "on the board");
            let off: Vec<f64> = ["x", "y", "z"].iter().map(|a| to.get(&format!("Offset.{a}")).unwrap().as_f64().unwrap()).collect();
            assert!(off.iter().zip(p).all(|(a, b)| (a - b).abs() < 1e-6), "{}: offset {off:?}, wanted {p:?}", c.name);
        }
        println!("{}: fx.cave.{} {:?} ({:?}, cooldown {}), {} attachment(s)", fxmod::cave_display(prefix), c.name, c.look, c.usage, c.cooldown, points.len());
    }
    identities.sort();
    identities.dedup();
    assert_eq!(identities.len(), CAVE.len(), "each effect item has its own InstanceGuidHash");

    // the graphic: in the holo host's bundle (since 2026-10-22: The Caveman owns its mask, normal map and holo map
    // too), whole, and exactly the plain white chain
    let name = fxmod::cave_texture_name(prefix);
    let (header, chunk) = texture_in_mod_of(layer, build::Host::Holo, &name);
    for slot in [build::SLOT_OSK, build::SLOT_NY, build::SLOT_RGB] {
        let layer_name = name.replace(build::SLOT_ART, slot);
        let (h, c) = texture_in_mod_of(layer, build::Host::Holo, &layer_name);
        assert_eq!((h.format, h.width, h.height, h.mips, h.first_mip), (66, 512, 2048, 12, 0), "{layer_name}: whole BC7 linear 512 x 2048");
        assert_eq!(c.len(), h.chunk_size as usize);
        println!("{layer_name}: {} bytes (The Caveman's own layer)", c.len());
    }
    assert_eq!((header.format, header.width, header.height, header.mips, header.first_mip), (67, 512, 2048, 12, 0), "whole BC7 sRGB 512 x 2048");
    assert_eq!(chunk.len(), header.chunk_size as usize);
    assert!(chunk == fxmod::cave_mips(), "The Caveman's texture is the plain white chain board reset uploads");
    println!("{name}: {} bytes, plain white, as cave_mips()", chunk.len());
}

/// A texture a mod added to the plain host's bundle: its header and its decoded chunk.
fn texture_in_mod(layer: &Layer, name: &str) -> (texture::TextureHeader, Vec<u8>) {
    texture_in_mod_of(layer, build::Host::Plain, name)
}

/// A texture a mod added to a host's bundle: its header and its decoded chunk.
fn texture_in_mod_of(layer: &Layer, host: build::Host, name: &str) -> (texture::TextureHeader, Vec<u8>) {
    let toc = layer.toc(ITEMS_TOC).unwrap();
    let host = host.bundle();
    let b = toc.bundles.iter().find(|b| b.name == host).unwrap_or_else(|| panic!("no {host} in the mod"));
    let (manifest, files) = layer.bundle(b.region.as_ref().unwrap()).unwrap();
    let ri = manifest.res_index(name).unwrap_or_else(|| panic!("no texture {name}"));
    let res = &manifest.res[ri];
    assert_eq!(res.rid & 1, 1, "{name}: an odd resource id");
    let header = texture::TextureHeader::parse(&layer.decode(&files[manifest.res_file(ri) - 1]).unwrap(), &res.meta).unwrap();
    let guid = toc::Guid::from_le_bytes(header.chunk);
    let ci = manifest.chunks.iter().position(|c| c.guid == guid).unwrap_or_else(|| panic!("{name}: its chunk is not in the bundle"));
    let chunk = layer.decode(&files[manifest.chunk_file(ci) - 1]).unwrap();
    (header, chunk)
}

/// What the trainer's `board art` uploads for a PNG (`fxmod::board_art`) is byte for byte what `caveman` writes into a
/// mod for the same PNG: two decks built by the engine (one with a colour behind its transparent parts, one on natural
/// wood), their art chunks read back from the mod's archive.
#[test]
#[ignore]
fn board_art_matches_the_build() {
    let game = Arc::new(Game::open(Path::new(r"E:\Games\ReSkate")).unwrap());
    let mods = scratch().join("board").join("art-mods");
    std::fs::create_dir_all(&mods).unwrap();
    let wood = build::natural_wood(&game).unwrap();
    // a sideways picture with transparent parts: turned upright, cropped, and composited
    let mut img = Image::new(900, 260, 4);
    for y in 0..img.height {
        for x in 0..img.width {
            let a = if (x / 60 + y / 40) % 3 == 0 { 0 } else { ((x * 255) / img.width) as u8 };
            img.pixel_mut(x, y).copy_from_slice(&[(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8, a]);
        }
    }
    let cases: [(&str, Option<[u8; 3]>); 2] = [("000 - Board Art Painted", Some([10, 200, 30])), ("000 - Board Art Wood", None)];
    let mut decks = Vec::new();
    for (display, paint) in cases {
        let (art, _) = build::fit_art_on(&img, &wood, paint).unwrap();
        decks.push(build::DeckSpec::new(display, fxmod::deck_key(display), art, build::DeckLayers::Effects(effects::DeckEffects::default())));
    }
    let opts = build::BuildOptions { item_cache: false, ..build::BuildOptions::default() };
    build::build(game.clone(), &decks, &[], &[], &[], &mods, "Board-Art-Test", &opts, &mut |l| println!("{l}")).unwrap();
    let layer = Layer::open(&mods.join("Board-Art-Test"), game.clone());
    for (display, paint) in cases {
        let name = format!("{}{}_role3_{}", build::PREFIX, build::base_name(&fxmod::deck_key(display)), build::SLOT_ART);
        let (_, chunk) = texture_in_mod(&layer, &name);
        let (mine, notes) = fxmod::board_art(&img, &wood, paint).unwrap();
        assert_eq!(mine.len(), chunk.len(), "{display}");
        assert!(mine == chunk, "{display}: board_art writes what the build writes");
        println!("{display}: {} bytes, byte for byte the build's ({})", chunk.len(), notes.join("; "));
    }
}
