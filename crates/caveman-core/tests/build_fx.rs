//! Builds the Caveman-FX mod (fxmod.rs) into the session scratchpad and reads it back from its own archive: the fx
//! states (BoolAssets in the game's GUID style), each test deck's item, its effect asset firing on its fx state
//! (TriggerType 6, usage, cooldown, the state imported by GUID), the blueprints, and the master item list. Needs the
//! game (read only); run with `--ignored`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use caveman_core::build::{self, MASTER, SHARED};
use caveman_core::ebx::{self, Document, Guid, Pointer};
use caveman_core::effects::Usage;
use caveman_core::fxmod::{self, FxLook, COMBOS, DECKS, STATES};
use caveman_core::game::{Game, Layer, ITEMS_TOC};

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
    assert_eq!(built.report.decks.len(), DECKS.len() + COMBOS.len());

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
    for short in STATES {
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
    println!("verified {} fx states, {} decks and {} combo deck(s) in {}", STATES.len(), DECKS.len(), COMBOS.len(), mods.join(fxmod::MOD).display());
}
