//! The EBX edits Caveman's build makes (newdeck.build, vfx.py), done with the Rust API and compared byte for byte
//! with Python's (the scratchpad's `ebxport/edit_fixtures.py` writes `edits.pack`; this file mirrors it step for
//! step, with the same deterministic GUIDs: the k-th new GUID is k as 16 little-endian bytes). Run with `--ignored`.

use std::collections::HashMap;
use std::path::PathBuf;

use caveman_core::ebx::{self, Data, Document, Guid, Object, Pointer, ResourceRef, Value};

fn corpus_dir() -> PathBuf {
    std::env::var_os("CAVEMAN_EBX_CORPUS").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(r"C:\Users\thetr\AppData\Local\Temp\claude\C--Users-thetr-Projects-ReSkate\c6d3c964-296a-4f0b-bedc-241e3cf960f6\scratchpad\ebxport\corpus")
    })
}

fn read_pack(name: &str) -> Vec<(String, Vec<u8>)> {
    let data = std::fs::read(corpus_dir().join(name)).unwrap();
    let mut out = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let n = u32::from_le_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        let name = String::from_utf8(data[at + 4..at + 4 + n].to_vec()).unwrap();
        at += 4 + n;
        let m = u32::from_le_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        out.push((name, data[at + 4..at + 4 + m].to_vec()));
        at += 4 + m;
    }
    out
}

const PEARL: &str = "b7d407a4-b44c-42e2-b91e-877d817c872d";
const FROST: &str = "e04615d5-ea8d-416e-acc5-a3b1b9f989e8";
const TRAIL_GRAPH: &str = "b48e76d9-4a88-46c6-a49f-26b2b1ae3d70";
const HELPER_GRAPH: &str = "bf1101c3-b8cc-11ef-b925-bbfa60fddb5c";
const TRAIL_COLOUR: i64 = -1752990270;
const TRAIL_COLOUR2: i64 = -1752990271;
const WIDTH: i64 = 226981187;

struct Guids(u128);

impl Guids {
    fn next(&mut self) -> Guid {
        self.0 += 1;
        Guid(self.0.to_le_bytes())
    }

    /// newdeck._clone: new file and instance GUIDs, renamed.
    fn clone_doc(&mut self, data: &[u8], name: &str) -> Document {
        let mut doc = ebx::read_document(data).unwrap();
        doc.set_file_guid(self.next());
        for i in 0..doc.instances.len() {
            let g = self.next();
            doc.set_instance_guid(i, g);
        }
        doc.rename(name).unwrap();
        doc
    }
}

fn write(doc: &Document) -> Vec<u8> {
    ebx::write_document_with(doc, false, false).unwrap()
}

fn djb32(text: &str) -> u64 {
    let mut h: u32 = 5381;
    for c in text.bytes() {
        h = h.wrapping_mul(33) ^ c as u32;
    }
    h as u64
}

/// vfx._set_vec: a Vec3/Vec4 field's x, y, z(, w) set to floats.
fn set_vec(obj: &mut Object, field: &str, values: &[f64]) {
    let vec = obj.find_mut(field).unwrap().value.data.as_object_mut().unwrap();
    for (name, v) in ["x", "y", "z", "w"].iter().zip(values) {
        vec.find_mut(name).unwrap().value.data = Data::F64(*v);
    }
}

fn graph_file(doc: &Document, obj: &Object) -> Option<Guid> {
    match obj.find("EmitterGraph").unwrap().value.data.as_pointer().unwrap() {
        Pointer::External(i) => Some(doc.imports[i].0),
        _ => None,
    }
}

fn internal(ptr: &Value) -> usize {
    match ptr.data.as_pointer().unwrap() {
        Pointer::Internal(i) => i,
        other => panic!("not an internal pointer: {other:?}"),
    }
}

/// Edits the EffectEntityData's Components of a blueprint: `decide(doc, instance, kept so far)` returns whether to
/// keep a component, after editing it.
fn edit_components(doc: &mut Document, mut decide: impl FnMut(&mut Document, usize, usize) -> bool) {
    let effect = doc.instances_of("EffectEntityData")[0];
    let components: Vec<Value> = doc.object(effect).unwrap().find("Components").unwrap().value.data.as_array().unwrap().clone();
    let mut keep = Vec::new();
    for v in components {
        if decide(doc, internal(&v), keep.len()) {
            keep.push(v);
        }
    }
    *doc.object_mut(effect).unwrap().find_mut("Components").unwrap().value.data.as_array_mut().unwrap() = keep;
}

#[test]
#[ignore]
fn build_edits_match_python() {
    let t: HashMap<String, Vec<u8>> = read_pack("edits-templates.pack").into_iter().collect();
    let expected: Vec<(String, Vec<u8>)> = read_pack("edits.pack");
    let mut g = Guids(0);
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    let p = "characters/customization/reskate/board_bottomart/";
    let base = "000_-_caveman_parity";

    // texture and its resource id
    let mut tex = g.clone_doc(&t["tex"], &format!("{p}{base}_role3_2259d419-387f-b1ba-30fc-c5140100b012"));
    tex.set("Resource", ResourceRef::new(0x1234567890ABCDEF), 0).unwrap();
    out.push(("texture".into(), write(&tex)));
    // appearance preset, pointed at the new texture
    let t_tex = ebx::read_document(&t["tex"]).unwrap();
    let mut ap = g.clone_doc(&t["ap"], &format!("{p}{base}_ap"));
    ap.remap_imports(&HashMap::from([
        (t_tex.file_guid, tex.file_guid),
        (t_tex.instances[0].instance_guid, tex.instances[0].instance_guid),
    ]));
    out.push(("ap".into(), write(&ap)));
    // holo preset with the foil mode switched (Pearl -> Frost)
    let mut hap = g.clone_doc(&t["holo_ap"], &format!("{p}{base}_holo_ap"));
    hap.remap_imports(&HashMap::from([(Guid::parse(PEARL).unwrap(), Guid::parse(FROST).unwrap())]));
    out.push(("holo_ap".into(), write(&hap)));
    // thumbnail and its CDN record
    let t_thumb = ebx::read_document(&t["thumb"]).unwrap();
    let mut th = g.clone_doc(&t["thumb"], &format!("thumbnail/tool/own_{base}"));
    th.set("Resource", ResourceRef::new(0x1111222233334445), 0).unwrap();
    out.push(("thumb".into(), write(&th)));
    let cdn_name = format!("thumbnail/cdn/img_own_{base}");
    let mut cdn = g.clone_doc(&t["cdn"], &cdn_name);
    cdn.remap_imports(&HashMap::from([
        (t_thumb.file_guid, th.file_guid),
        (t_thumb.instances[0].instance_guid, th.instances[0].instance_guid),
    ]));
    cdn.set("NameHash", Data::UInt(djb32(&cdn_name)), 0).unwrap();
    out.push(("cdn".into(), write(&cdn)));

    // board effects: an aura blueprint (light and trails recoloured, helper dropped) and a nyan one (one trail)
    let colour = [0.0, 1.0, 1.0];
    let (trail, helper) = (Guid::parse(TRAIL_GRAPH).unwrap(), Guid::parse(HELPER_GRAPH).unwrap());
    let mut bp = g.clone_doc(&t["bp"], &format!("characters/vfx/caveman/ebp_{base}_aura"));
    edit_components(&mut bp, |doc, i, _| {
        if doc.instance_type(i) == "EmitterGraphEntityData" {
            let gf = graph_file(doc, doc.object(i).unwrap());
            if gf == Some(helper) {
                return false;
            }
            if gf == Some(trail) {
                let o = doc.object_mut(i).unwrap();
                set_vec(o.find_mut("Transform").unwrap().value.data.as_object_mut().unwrap(), "trans", &[0.0, 0.0, 0.0]);
                for prm in o.find_mut("EmitterGraphParams").unwrap().value.data.as_array_mut().unwrap() {
                    let po = prm.data.as_object_mut().unwrap();
                    let pid = po.find("PropertyId").unwrap().value.data.as_i64().unwrap();
                    if pid == TRAIL_COLOUR {
                        set_vec(po, "Value", &[colour[0] * 1.5, colour[1] * 1.5, colour[2] * 1.5, 0.0]);
                    } else if pid == TRAIL_COLOUR2 {
                        set_vec(po, "Value", &[colour[0] * 0.25, colour[1] * 0.25, colour[2] * 0.25, 0.0]);
                    }
                }
            }
        }
        true
    });
    let light = bp.instances_of("PbrSphereLightEntityData")[0];
    let lo = bp.object_mut(light).unwrap();
    set_vec(lo, "Color", &colour);
    lo.set("Intensity", Data::F64(40.0)).unwrap();
    lo.set("AttenuationRadius", Data::F64(0.9)).unwrap();
    out.push(("aura_blueprint".into(), write(&bp)));

    let mut nb = g.clone_doc(&t["bp"], &format!("characters/vfx/caveman/ebp_{base}_nyan"));
    edit_components(&mut nb, |doc, i, kept| {
        if doc.instance_type(i) != "EmitterGraphEntityData" || kept > 0 {
            return false;
        }
        if graph_file(doc, doc.object(i).unwrap()) != Some(trail) {
            return false;
        }
        let o = doc.object_mut(i).unwrap();
        set_vec(o.find_mut("Transform").unwrap().value.data.as_object_mut().unwrap(), "trans", &[0.0, 0.0, 0.0]);
        for prm in o.find_mut("EmitterGraphParams").unwrap().value.data.as_array_mut().unwrap() {
            let po = prm.data.as_object_mut().unwrap();
            let pid = po.find("PropertyId").unwrap().value.data.as_i64().unwrap();
            if pid == TRAIL_COLOUR || pid == TRAIL_COLOUR2 {
                set_vec(po, "Value", &[7.0, 1.0, 0.5, 0.0]);
            } else if pid == WIDTH {
                po.set("Value.x", Data::F64(0.28)).unwrap();
            }
        }
        for q in ["Low", "Medium", "High", "Ultra"] {
            o.set(&format!("EmitterGraphOverrides.ParticleLifeSpan.{q}"), Data::F64(1.2)).unwrap();
        }
        true
    });
    out.push(("nyan_blueprint".into(), write(&nb)));

    // the effect asset: attachments on the board, one per truck (aura) and just one (nyan)
    let mut aura_asset_ref = None;
    for (label, points, blueprint) in [
        ("aura_asset", vec![[0.0, 0.03, 0.205], [0.0, 0.03, -0.205]], &bp),
        ("nyan_asset", vec![[0.0, 0.03, 0.0]], &nb),
    ] {
        let mut va = g.clone_doc(&t["va"], &format!("characters/vfx/caveman/{base}_{label}"));
        let items = va.instances_of("SkaterVfxTriggerableItem");
        let used: Vec<usize> = items[..points.len()].to_vec();
        let effect_item = va.instances_of("SkaterVfxEffectItem")[0];
        let effects = va.object_mut(effect_item).unwrap().find_mut("Effects").unwrap().value.data.as_array_mut().unwrap();
        effects.retain(|v| used.contains(&internal(v)));
        for (&i, point) in used.iter().zip(&points) {
            let effect_import = match va.object(i).unwrap().get("Effect").unwrap().as_pointer().unwrap() {
                Pointer::External(k) => k,
                other => panic!("{other:?}"),
            };
            va.imports[effect_import] = blueprint.reference();
            let o = va.object_mut(i).unwrap();
            set_vec(o, "Offset", point);
            set_vec(o, "Rotation", &[0.0, 0.0, 0.0]);
            o.set("AttachType", Data::Int(1)).unwrap();
            o.set("OffsetType", Data::Int(1)).unwrap();
            o.set("TargetBoneHash", Data::UInt(0xFFFF_FFFF)).unwrap();
            o.set("MirroredTargetBoneHash", Data::UInt(0xFFFF_FFFF)).unwrap();
        }
        for i in va.instances_of("SkaterVfxEffectItem") {
            va.object_mut(i).unwrap().set("InstanceGuidHash", Data::Int(-0x123456789ABCDEF)).unwrap();
        }
        out.push((label.into(), write(&va)));
        if label == "aura_asset" {
            aura_asset_ref = Some(va.reference());
        }
    }

    // the item: key, hash, thumbnails' CDN records, preset name, board effect
    let t_cdn = ebx::read_document(&t["cdn"]).unwrap();
    let key = format!("Own_{base}");
    let mut item = g.clone_doc(&t["item"], &format!("items/board_bottomart/own_{base}"));
    item.set("Key", key.as_str(), 0).unwrap();
    item.set("HashedAssetKey", Data::UInt(djb32(&key)), 0).unwrap();
    item.remap_imports(&HashMap::from([
        (t_cdn.file_guid, cdn.file_guid),
        (t_cdn.instances[0].instance_guid, cdn.instances[0].instance_guid),
    ]));
    let di = item.instances_of("DelMarCustomizationItemData")[0];
    item.set("AssetPaths[0].AssetName", format!("{base}_ap"), di).unwrap();
    let ptr = item.add_import(aura_asset_ref.unwrap());
    item.set("SkaterVfx", ptr, di).unwrap();
    item.set("PreviewVFX", true, di).unwrap();
    out.push(("item".into(), write(&item)));

    // the master item list: one more entry
    let mut master = ebx::read_document(&t["master"]).unwrap();
    let ptr = master.add_import(item.reference());
    master.object_mut(0).unwrap().find_mut("Items").unwrap().value.data.as_array_mut().unwrap().push(Value::new(ptr));
    out.push(("master".into(), write(&master)));
    out.push(("master_preserved".into(), ebx::write_document(&master).unwrap()));

    assert_eq!(g.0, 45, "GUIDs used (Python used 45)");
    let mut failures = Vec::new();
    for ((label, rust), (plabel, python)) in out.iter().zip(&expected) {
        assert_eq!(label, plabel);
        if rust != python {
            let at = rust.iter().zip(python).position(|(a, b)| a != b).unwrap_or(rust.len().min(python.len()));
            failures.push(format!("{label}: first difference at {at:#x} (python {} rust {})", python.len(), rust.len()));
        }
    }
    println!("{} edited documents, {} different from Python", out.len(), failures.len());
    for f in &failures {
        println!("   {f}");
    }
    assert_eq!(out.len(), expected.len());
    assert!(failures.is_empty());
}
