//! Writing EBX (ebx.py `write_document` and `_Builder`).

use std::collections::{BTreeSet, HashMap};

use super::*;

/// The document as EBX bytes, keeping the file's layout: byte for byte as read when nothing changed
/// (ebx.py `write_document(doc)`).
pub fn write_document(doc: &Document) -> Result<Vec<u8>> {
    write_document_with(doc, false, true)
}

/// ebx.py `write_document(doc, refuse_new_array_hashes, preserve_layout)`. `preserve_layout` keeps instances,
/// arrays and sections no earlier than the file had them; false packs them tight (what the build uses for new
/// assets). `refuse_new_array_hashes` fails when a new array has no array of its field to take a hash from.
pub fn write_document_with(doc: &Document, refuse_new_array_hashes: bool, preserve_layout: bool) -> Result<Vec<u8>> {
    Builder::new(doc, refuse_new_array_hashes, preserve_layout).run()
}

/// Where a pointer in the old payload led (ebx.py `_Builder.old_target`).
enum Target {
    Array(usize),
    String(usize),
    Instance(usize),
    Boxed(usize),
    Empty,
    Unknown,
}

/// An array to place: a document value, or a record only boxed values reach.
enum Placement<'a> {
    Tree(&'a Value, usize, usize),
    Carried(ArrayRecord),
}

/// A string table key (ebx.py `write_strings`): a text the file had at an offset, or a new text.
#[derive(Clone, PartialEq, Eq, Hash)]
enum StrKey {
    File(usize, Vec<u8>),
    New(Vec<u8>),
}

impl StrKey {
    fn text(&self) -> &[u8] {
        match self {
            StrKey::File(_, t) | StrKey::New(t) => t,
        }
    }
}

struct Builder<'a> {
    doc: &'a Document,
    refuse: bool,
    preserve: bool,
    p: Vec<u8>,
    instance_offsets: Vec<usize>,
    pointer_offsets: Vec<usize>,
    resource_offsets: Vec<usize>,
    import_offsets: Vec<usize>,
    type_info_offsets: Vec<usize>,
    array_slots: Vec<(usize, Option<&'a Value>)>,
    strings_pending: Vec<(usize, Vec<u8>, Option<usize>)>,
    pointers_pending: Vec<(usize, Pointer)>,
    boxes_pending: Vec<(usize, BoxedRef)>,
    array_data: HashMap<*const Value, usize>,
    arrays_out: Vec<ArrayRecord>,
    boxed_out: Vec<ArrayRecord>,
    arrays_offset: usize,
    boxed_offset: usize,
    empty_array_target: usize,
    old_arrays: HashMap<usize, ArrayRecord>,
    old_instances: HashMap<usize, usize>,
    carried: HashMap<usize, usize>,
    boxed_targets: Vec<(usize, usize)>,
    known_hashes: Option<HashMap<(usize, usize), u32>>,
}

fn sorted_unique(values: &[usize]) -> Vec<usize> {
    values.iter().copied().collect::<BTreeSet<_>>().into_iter().collect()
}

impl<'a> Builder<'a> {
    fn new(doc: &'a Document, refuse: bool, preserve: bool) -> Builder<'a> {
        Builder {
            doc,
            refuse,
            preserve,
            p: Vec::new(),
            instance_offsets: Vec::new(),
            pointer_offsets: Vec::new(),
            resource_offsets: Vec::new(),
            import_offsets: Vec::new(),
            type_info_offsets: Vec::new(),
            array_slots: Vec::new(),
            strings_pending: Vec::new(),
            pointers_pending: Vec::new(),
            boxes_pending: Vec::new(),
            array_data: HashMap::new(),
            arrays_out: Vec::new(),
            boxed_out: Vec::new(),
            arrays_offset: 0,
            boxed_offset: 0,
            empty_array_target: 0,
            old_arrays: HashMap::new(),
            old_instances: HashMap::new(),
            carried: HashMap::new(),
            boxed_targets: Vec::new(),
            known_hashes: None,
        }
    }

    /// With preserve_layout, nothing moves ahead of where the file had it: dead space a tool left (ReSkate Studio
    /// keeps the room of array elements it removed) stays, so an unchanged document writes back byte for byte. What
    /// grew simply flows past. The gap keeps the file's own bytes; nothing points into it (ebx.py `at_least`).
    fn at_least(&mut self, computed: usize, source: usize) -> usize {
        if !(self.preserve && source != 0 && source > computed) {
            return computed;
        }
        let end = source.min(self.doc.payload.len());
        if end > self.p.len() {
            let from = self.p.len();
            self.p.extend_from_slice(&self.doc.payload[from..end]);
        }
        source
    }

    fn run(mut self) -> Result<Vec<u8>> {
        self.validate()?;
        self.write_instances()?;
        self.write_arrays()?;
        self.write_boxed_values()?;
        let len = self.p.len();
        let string_offset = self.at_least(len, self.doc.strings_offset);
        self.grow(string_offset);
        self.write_strings();
        self.resolve_pointers()?;
        let payload = std::mem::take(&mut self.p);
        let fixup = self.write_fixup(string_offset);
        let extra = self.write_extra();
        Ok(self.write_riff(&payload, &fixup, &extra))
    }

    fn validate(&self) -> Result<()> {
        let doc = self.doc;
        if doc.reflection_data.is_empty() {
            return err("EBX has no donor reflection table");
        }
        let mut internal = false;
        for inst in &doc.instances {
            internal |= !inst.exported;
            if internal && inst.exported {
                return err("exported EBX instances must precede internal instances");
            }
            if !matches!(inst.descriptor, Some(d) if d < doc.types.len()) {
                return err("EBX instance has no valid type descriptor");
            }
            if inst.fixup_type as usize >= doc.fixup_type_guids.len() {
                return err("EBX instance has no valid fixup type");
            }
        }
        Ok(())
    }

    fn grow(&mut self, end: usize) {
        if end > self.p.len() {
            self.p.resize(end, 0);
        }
    }

    fn put(&mut self, offset: usize, data: &[u8]) {
        self.grow(offset + data.len());
        self.p[offset..offset + data.len()].copy_from_slice(data);
    }

    /// A 32-bit little-endian value; negative displacements wrap (ebx.py `value & 0xFFFFFFFF`).
    fn u32(&mut self, offset: usize, value: i64) {
        self.put(offset, &(value as u32).to_le_bytes());
    }

    fn write_instances(&mut self) -> Result<()> {
        let doc = self.doc;
        let mut cursor = 0usize;
        for inst in &doc.instances {
            let descriptor = inst.descriptor.unwrap();
            let t = &doc.types[descriptor];
            let computed = align_up(cursor + if inst.exported { 16 } else { 0 }, (t.alignment as usize).max(1));
            let start = self.at_least(computed, inst.data_offset);
            if inst.exported {
                self.put(start - 16, &inst.instance_guid.0);
            }
            self.grow(start + t.size as usize);
            if !inst.raw_image.is_empty() {
                let n = inst.raw_image.len().min(t.size as usize);
                self.put(start, &inst.raw_image[..n]);
            }
            self.put(start, &inst.fixup_type.to_le_bytes());
            self.write_type(descriptor, start, inst.object.as_ref())?;
            self.instance_offsets.push(start);
            cursor = start + t.size as usize;
        }
        Ok(())
    }

    fn write_type(&mut self, type_index: usize, start: usize, obj: Option<&'a Object>) -> Result<()> {
        let doc = self.doc;
        let t = &doc.types[type_index];
        self.grow(start + t.size as usize);
        // ebx.py `{f.descriptor: f.value for f in obj.fields}`: the last value of a descriptor wins
        let mut values: HashMap<usize, &'a Value> = HashMap::new();
        if let Some(obj) = obj {
            for f in &obj.fields {
                values.insert(f.descriptor, &f.value);
            }
        }
        for k in 0..t.field_count as usize {
            let di = t.field_index as usize + k;
            let f = &doc.fields[di];
            if f.type_() == INHERITED {
                self.write_type(f.class_ref as usize, start, obj)?;
                continue;
            }
            let pos = start + f.data_offset as usize;
            let value = values.get(&di).copied();
            if f.category() == CAT_ARRAY {
                self.grow(pos + 8);
                self.pointer_offsets.push(pos);
                self.array_slots.push((pos, value));
            } else if f.type_() == STRUCTURE {
                let nested_start = align_up(pos, (doc.types[f.class_ref as usize].alignment as usize).max(1));
                let nested = value.and_then(|v| v.data.as_object());
                self.write_type(f.class_ref as usize, nested_start, nested)?;
            } else {
                self.write_scalar(f, pos, value)?;
            }
        }
        Ok(())
    }

    fn write_scalar(&mut self, f: &FieldDescriptor, pos: usize, value: Option<&'a Value>) -> Result<()> {
        let none = Data::None;
        let data = value.map(|v| &v.data).unwrap_or(&none);
        let t = f.type_();
        match t {
            BOOL => self.put(pos, &[data.truthy() as u8]),
            INT8 | UINT8 | INT16 | UINT16 | INT32 | ENUM | UINT32 | INT64 | UINT64 => {
                let bytes = int_bytes(t, data)?;
                self.put(pos, &bytes);
            }
            FLOAT32 => {
                let bits = match *data {
                    Data::F32(v) => v.to_bits(),
                    Data::F64(v) => (v as f32).to_bits(),
                    Data::Int(v) => (v as f32).to_bits(),
                    Data::UInt(v) => (v as f32).to_bits(),
                    Data::Bool(v) => (v as u8 as f32).to_bits(),
                    _ => 0f32.to_bits(),
                };
                self.put(pos, &bits.to_le_bytes());
            }
            FLOAT64 => {
                let bits = match *data {
                    Data::F64(v) => v.to_bits(),
                    Data::F32(v) => (v as f64).to_bits(),
                    Data::Int(v) => (v as f64).to_bits(),
                    Data::UInt(v) => (v as f64).to_bits(),
                    Data::Bool(v) => (v as u8 as f64).to_bits(),
                    _ => 0f64.to_bits(),
                };
                self.put(pos, &bits.to_le_bytes());
            }
            GUID => {
                let bytes = match data {
                    Data::Guid(g) => g.0,
                    _ => [0u8; 16],
                };
                self.put(pos, &bytes);
            }
            SHA1 => {
                let bytes = match data {
                    Data::Sha1(s) => *s,
                    _ => [0u8; 20],
                };
                self.put(pos, &bytes);
            }
            STRING => {
                let raw: &[u8] = match data {
                    Data::Str(s) => s,
                    _ => &[],
                };
                if raw.len() > 31 {
                    return err("EBX fixed string exceeds 31 bytes");
                }
                let mut buf = [0u8; 32];
                buf[..raw.len()].copy_from_slice(raw);
                self.put(pos, &buf);
            }
            CSTRING | FILEREF => {
                self.put(pos, &[0u8; 8]);
                self.pointer_offsets.push(pos);
                let text = match data {
                    Data::Str(s) => s.clone(),
                    _ => Vec::new(),
                };
                self.strings_pending.push((pos, text, value.and_then(|v| v.source_offset)));
            }
            RESOURCEREF => {
                let (id, listed) = match data {
                    Data::ResourceRef(r) => (r.id, r.listed),
                    _ => (0, None),
                };
                self.put(pos, &id.to_le_bytes());
                let is_ref = matches!(data, Data::ResourceRef(_));
                if !(self.preserve && is_ref && listed == Some(false)) {
                    self.resource_offsets.push(pos);
                }
            }
            POINTER => {
                self.put(pos, &[0u8; 8]);
                let ptr = data.as_pointer().unwrap_or(Pointer::Null);
                self.pointers_pending.push((pos, ptr));
            }
            TYPEREF | DELEGATE => {
                let encoded = match data {
                    Data::TypeRef(r) => r.encoded,
                    _ => 0,
                };
                let mut buf = [0u8; 8];
                buf[..4].copy_from_slice(&encoded.to_le_bytes());
                self.put(pos, &buf);
                if encoded != 0 && t == TYPEREF {
                    self.type_info_offsets.push(pos);
                }
            }
            BOXEDVALUEREF => {
                self.grow(pos + 16);
                let boxed = match data {
                    Data::Boxed(b) if b.encoded_type != 0 => *b,
                    _ => {
                        self.put(pos, &[0u8; 16]);
                        return Ok(());
                    }
                };
                let mut buf = [0u8; 8];
                buf[..4].copy_from_slice(&boxed.encoded_type.to_le_bytes());
                self.put(pos, &buf);
                self.type_info_offsets.push(pos);
                self.pointer_offsets.push(pos + 8);
                self.boxes_pending.push((pos, boxed));
            }
            _ => self.grow(pos + scalar_size(t)),
        }
        Ok(())
    }

    fn element_stride(&self, f: &FieldDescriptor) -> usize {
        if f.type_() == STRUCTURE {
            return (self.doc.types[f.class_ref as usize].size as usize).max(1);
        }
        scalar_size(f.type_())
    }

    fn element_alignment(&self, f: &FieldDescriptor) -> usize {
        if f.type_() == STRUCTURE {
            return (self.doc.types[f.class_ref as usize].alignment as usize).max(1);
        }
        scalar_alignment(f.type_())
    }

    fn fixup_slot(&self, descriptor: usize) -> u16 {
        let doc = self.doc;
        let Some(t) = doc.types.get(descriptor) else {
            return 0xFFFF;
        };
        let n = doc.fixup_type_guids.len().min(doc.fixup_type_signatures.len());
        for i in 0..n {
            if doc.fixup_type_guids[i] == t.guid && doc.fixup_type_signatures[i] == t.signature {
                return i as u16;
            }
        }
        0xFFFF
    }

    fn array_hash(&self, f: &FieldDescriptor) -> u32 {
        let raw_type = ((f.encoded_type as u32) << 1) as u16;
        let class_ref = if f.type_() == STRUCTURE { self.fixup_slot(f.class_ref as usize) } else { 0xFFFF };
        for a in &self.doc.arrays {
            if a.encoded_type == raw_type && a.class_ref == class_ref {
                return a.hash;
            }
        }
        self.doc.arrays.first().map(|a| a.hash).unwrap_or(f.name_hash)
    }

    fn new_array_hash(&mut self, holder: usize, descriptor: usize, f: &FieldDescriptor) -> Result<u32> {
        if self.known_hashes.is_none() {
            let mut known = HashMap::new();
            for inst in &self.doc.instances {
                if let Some(obj) = &inst.object {
                    collect_hashes(obj, &mut known);
                }
            }
            self.known_hashes = Some(known);
        }
        let found = self.known_hashes.as_ref().unwrap().get(&(holder, descriptor)).copied();
        if self.refuse {
            let Some(h) = found else {
                return err(format!("EBX new array {} has no array of its field to take a hash from", f.name));
            };
            if self.doc.hashed_reflection_names && self.doc.reflection_groups.iter().all(|g| g.0 != h) {
                return err(format!("EBX array hash of {} is not in the reflection's group table", f.name));
            }
        }
        Ok(found.unwrap_or_else(|| self.array_hash(f)))
    }

    /// Every non-empty array, in the order ReSkate's writer emits them: the instances' arrays in field order, each
    /// followed depth-first by the arrays inside its elements. Arrays read from a file then move to their original
    /// order (the game's own files place nested arrays first) (ebx.py `array_order`).
    fn array_order(&self) -> Vec<(&'a Value, usize, usize)> {
        let doc: &'a Document = self.doc;
        let mut seq = Vec::new();
        fn visit<'b>(doc: &'b Document, obj: &'b Object, seq: &mut Vec<(&'b Value, usize, usize)>) {
            for fv in &obj.fields {
                let v = &fv.value;
                if doc.fields[fv.descriptor].category() == CAT_ARRAY {
                    if let Data::Array(items) = &v.data {
                        if !items.is_empty() {
                            seq.push((v, fv.descriptor, obj.descriptor));
                            for e in items {
                                if let Data::Object(o) = &e.data {
                                    visit(doc, o, seq);
                                }
                            }
                        }
                    }
                } else if let Data::Object(o) = &v.data {
                    visit(doc, o, seq);
                }
            }
        }
        for inst in &doc.instances {
            if let Some(obj) = &inst.object {
                visit(doc, obj, &mut seq);
            }
        }
        let mut known: Vec<_> = seq.iter().filter(|a| a.0.source_offset.is_some()).copied().collect();
        known.sort_by_key(|a| a.0.source_offset.unwrap());
        known.extend(seq.iter().filter(|a| a.0.source_offset.is_none()).copied());
        known
    }

    fn write_arrays(&mut self) -> Result<()> {
        let computed = align_up(self.p.len(), 16);
        self.arrays_offset = self.at_least(computed, self.doc.arrays_offset);
        self.grow(self.arrays_offset + 32);
        self.empty_array_target = self.arrays_offset + 16;
        let tree = self.array_order();
        let carried = self.boxed_array_records();
        if self.preserve {
            // Arrays from the file, the tree's and the ones only boxed values reach, in file order.
            let mut known: Vec<(usize, u8, Placement<'a>)> = Vec::new();
            for &(v, d, h) in &tree {
                if let Some(s) = v.source_offset {
                    known.push((s, 0, Placement::Tree(v, d, h)));
                }
            }
            for r in &carried {
                known.push((r.offset, 1, Placement::Carried(*r)));
            }
            known.sort_by_key(|k| (k.0, k.1));
            for (_, _, item) in known {
                match item {
                    Placement::Tree(v, d, h) => self.place_array(v, d, h)?,
                    Placement::Carried(r) => self.carry_array(r)?,
                }
            }
            for &(v, d, h) in &tree {
                if v.source_offset.is_none() {
                    self.place_array(v, d, h)?;
                }
            }
        } else {
            for &(v, d, h) in &tree {
                self.place_array(v, d, h)?;
            }
            for r in carried {
                self.carry_array(r)?;
            }
        }
        let slots = std::mem::take(&mut self.array_slots);
        for (slot, value) in &slots {
            let placed = value.and_then(|v| self.array_data.get(&(v as *const Value)).copied());
            let target = placed.unwrap_or(self.empty_array_target);
            self.u32(*slot, target as i64 - *slot as i64);
        }
        self.array_slots = slots;
        Ok(())
    }

    // What a boxed value points at is not in the document's values: its bytes are carried as they were, with the
    // arrays and strings its pointers reach.
    fn old_target(&self, slot: usize) -> Target {
        let old = &self.doc.payload;
        let disp = match old.get(slot..slot + 4) {
            Some(b) => i32::from_le_bytes(b.try_into().unwrap()) as i64,
            None => return Target::Unknown,
        };
        let target = ((slot as i64 + disp) as u64 & 0xFFFF_FFFF) as usize;
        if self.old_arrays.contains_key(&target) {
            return Target::Array(target);
        }
        if self.doc.strings_offset <= target && target < old.len() {
            return Target::String(target);
        }
        if let Some(&i) = self.old_instances.get(&target) {
            return Target::Instance(i);
        }
        if self.doc.boxed_values_offset <= target && target < self.doc.strings_offset {
            return Target::Boxed(target);
        }
        if 4 <= target && target <= old.len() && old[target - 4..target] == [0, 0, 0, 0] {
            return Target::Empty;
        }
        Target::Unknown
    }

    fn old_slots(start: usize, length: usize, offsets: &[usize]) -> Vec<usize> {
        sorted_unique(offsets).into_iter().filter(|&v| start <= v && v < start + length).collect()
    }

    /// The array records boxed values' pointers reach, in the order ReSkate's writer carries them.
    fn boxed_array_records(&mut self) -> Vec<ArrayRecord> {
        let doc = self.doc;
        if doc.payload.is_empty() {
            return Vec::new();
        }
        for a in &doc.arrays {
            self.old_arrays.entry(a.offset).or_insert(*a);
        }
        for (i, inst) in doc.instances.iter().enumerate() {
            self.old_instances.entry(inst.data_offset).or_insert(i);
        }
        let mut sources: Vec<&BoxedValueRecord> = doc.boxed_values.iter().collect();
        sources.sort_by_key(|v| v.offset);
        let mut out = Vec::new();
        for source in sources {
            for slot in Self::old_slots(source.offset, source.raw_bytes.len(), &doc.pointer_offsets) {
                if let Target::Array(key) = self.old_target(slot) {
                    out.push(self.old_arrays[&key]);
                }
            }
        }
        out
    }

    fn array_element(&self, record: &ArrayRecord) -> Result<(usize, usize)> {
        let t = ((record.encoded_type >> 1) >> 4) & 0x1F;
        if t == STRUCTURE {
            let slot = record.class_ref as usize;
            let doc = self.doc;
            if slot < doc.fixup_type_guids.len() && slot < doc.fixup_type_signatures.len() {
                for ty in &doc.types {
                    if ty.guid == doc.fixup_type_guids[slot] && ty.signature == doc.fixup_type_signatures[slot] {
                        return Ok(((ty.size as usize).max(1), (ty.alignment as usize).max(1)));
                    }
                }
            }
            return err("EBX array record has an unknown struct type");
        }
        Ok((scalar_size(t), scalar_alignment(t)))
    }

    fn carry_array(&mut self, record: ArrayRecord) -> Result<()> {
        if self.carried.contains_key(&record.offset) {
            return Ok(());
        }
        let (stride, alignment) = self.array_element(&record)?;
        let length = stride * record.count as usize;
        if record.offset + length > self.doc.payload.len() {
            return err("EBX carried array exceeds EBXD");
        }
        let computed = align_up(self.p.len() + 4, alignment);
        let data = self.at_least(computed, record.offset);
        self.u32(data - 4, record.count as i64);
        let bytes = self.doc.payload[record.offset..record.offset + length].to_vec();
        self.put(data, &bytes);
        self.carried.insert(record.offset, data);
        self.arrays_out.push(ArrayRecord { offset: data, ..record });
        self.relocate(record.offset, length, data)
    }

    fn relocate(&mut self, old_start: usize, length: usize, new_start: usize) -> Result<()> {
        let doc = self.doc;
        let moved = |old: usize| new_start + (old - old_start);
        for old in Self::old_slots(old_start, length, &doc.resource_ref_offsets) {
            self.resource_offsets.push(moved(old));
        }
        for old in Self::old_slots(old_start, length, &doc.import_offsets) {
            self.import_offsets.push(moved(old));
        }
        for old in Self::old_slots(old_start, length, &doc.type_info_offsets) {
            self.type_info_offsets.push(moved(old));
        }
        for old in Self::old_slots(old_start, length, &doc.pointer_offsets) {
            let slot = moved(old);
            self.pointer_offsets.push(slot);
            if doc.payload.is_empty() {
                continue;
            }
            match self.old_target(old) {
                Target::Array(key) => {
                    let record = self.old_arrays[&key];
                    self.carry_array(record)?;
                    let at = self.carried[&key];
                    self.u32(slot, at as i64 - slot as i64);
                }
                Target::String(key) => {
                    let end = doc.payload[key..].iter().position(|&c| c == 0).map(|p| key + p).unwrap_or(doc.payload.len());
                    self.strings_pending.push((slot, doc.payload[key..end].to_vec(), Some(key)));
                }
                Target::Instance(i) => {
                    let at = self.instance_offsets[i];
                    self.u32(slot, at as i64 - slot as i64);
                }
                Target::Empty => {
                    let at = self.empty_array_target;
                    self.u32(slot, at as i64 - slot as i64);
                }
                Target::Boxed(key) => self.boxed_targets.push((slot, key)),
                Target::Unknown => {}
            }
        }
        Ok(())
    }

    fn place_array(&mut self, array: &'a Value, descriptor: usize, holder: usize) -> Result<()> {
        let values = array.data.as_array().unwrap();
        let f = &self.doc.fields[descriptor];
        // The game's own layout aligns the count word, then the data, to the element alignment; ReSkate's (and
        // Studio's) writer aligns only the data. An array read from a file goes back where it was whenever that is a
        // valid place; a new or moved one follows the game's rule.
        let alignment = self.element_alignment(f);
        let minimum = align_up(self.p.len() + 4, alignment);
        let data = match array.source_offset {
            Some(source) if self.preserve && source >= minimum => self.at_least(minimum, source),
            _ => align_up(align_up(self.p.len(), alignment) + 4, alignment),
        };
        let stride = self.element_stride(f);
        self.u32(data - 4, values.len() as i64);
        self.grow(data + stride * values.len());
        for (e, value) in values.iter().enumerate() {
            let start = data + e * stride;
            if f.type_() == STRUCTURE {
                self.write_type(f.class_ref as usize, start, value.data.as_object())?;
            } else {
                self.write_scalar(f, start, Some(value))?;
            }
        }
        let h = match array.array_hash {
            Some(h) => h,
            None => self.new_array_hash(holder, descriptor, f)?,
        };
        // Struct and enum arrays name their type's fixup slot (the game's files do it for enums too).
        let class_ref = if f.type_() == STRUCTURE || f.type_() == ENUM { self.fixup_slot(f.class_ref as usize) } else { 0xFFFF };
        self.arrays_out.push(ArrayRecord {
            offset: data,
            count: values.len() as u32,
            hash: h,
            encoded_type: ((f.encoded_type as u32) << 1) as u16,
            class_ref,
        });
        self.array_data.insert(array as *const Value, data);
        Ok(())
    }

    fn write_boxed_values(&mut self) -> Result<()> {
        let doc = self.doc;
        let computed = align_up(self.p.len(), 16);
        self.boxed_offset = self.at_least(computed, doc.boxed_values_offset);
        self.grow(self.boxed_offset);
        let mut moved: HashMap<usize, usize> = HashMap::new();
        let mut sources: Vec<&BoxedValueRecord> = doc.boxed_values.iter().collect();
        sources.sort_by_key(|v| v.offset);
        for source in sources {
            if source.offset < doc.boxed_values_offset {
                return err("EBX boxed record precedes its section");
            }
            let offset = self.boxed_offset + source.offset - doc.boxed_values_offset;
            self.put(offset, &source.raw_bytes);
            self.relocate(source.offset, source.raw_bytes.len(), offset)?;
            moved.insert(source.offset, offset);
            self.boxed_out.push(ArrayRecord {
                offset,
                count: source.count,
                hash: source.hash,
                encoded_type: source.encoded_type,
                class_ref: source.class_ref,
            });
        }
        for (slot, target) in std::mem::take(&mut self.boxed_targets) {
            let at = self.boxed_offset + (target - doc.boxed_values_offset);
            self.u32(slot, at as i64 - slot as i64);
        }
        for (slot, r) in self.boxes_pending.clone() {
            let mut buf = [0u8; 8];
            buf[..4].copy_from_slice(&r.encoded_type.to_le_bytes());
            self.put(slot, &buf);
            let relative = match usize::try_from(r.data_offset).ok().and_then(|o| moved.get(&o)) {
                Some(&m) => m as i64 - (slot as i64 + 8),
                None => r.relative_offset,
            };
            self.put(slot + 8, &relative.to_le_bytes());
        }
        Ok(())
    }

    /// ReSkate's writer keeps one copy of each text. Texts read from a file keep their original order and copies
    /// (some tools write a string per field, even empty ones); new texts share an existing copy when there is one,
    /// else follow in the order they were met (ebx.py `write_strings`).
    fn write_strings(&mut self) {
        let pending = std::mem::take(&mut self.strings_pending);
        let mut rank: HashMap<StrKey, (u8, usize, usize)> = HashMap::new();
        let mut keys: Vec<Option<StrKey>> = Vec::with_capacity(pending.len());
        for (index, (_, value, source)) in pending.iter().enumerate() {
            match source {
                Some(s) if self.preserve => {
                    let key = StrKey::File(*s, value.clone());
                    rank.entry(key.clone()).or_insert((0, *s, index));
                    keys.push(Some(key));
                }
                _ => keys.push(None),
            }
        }
        let mut by_text: HashMap<Vec<u8>, StrKey> = HashMap::new();
        let mut ordered: Vec<(StrKey, (u8, usize, usize))> = rank.iter().map(|(k, r)| (k.clone(), *r)).collect();
        ordered.sort_by_key(|e| e.1);
        for (key, _) in &ordered {
            by_text.entry(key.text().to_vec()).or_insert_with(|| key.clone());
        }
        for (index, (_, value, _)) in pending.iter().enumerate() {
            if keys[index].is_none() {
                let key = match by_text.get(value) {
                    Some(k) => k.clone(),
                    None => {
                        let k = StrKey::New(value.clone());
                        by_text.insert(value.clone(), k.clone());
                        rank.insert(k.clone(), (1, 0, index));
                        k
                    }
                };
                keys[index] = Some(key);
            }
        }
        let mut ordered: Vec<(StrKey, (u8, usize, usize))> = rank.into_iter().collect();
        ordered.sort_by_key(|e| e.1);
        let mut offsets: HashMap<StrKey, usize> = HashMap::new();
        // (file offset, its bytes, new offset) of the file's texts written so far
        let mut placed: Vec<(usize, Vec<u8>, usize)> = Vec::new();
        for (key, _) in ordered {
            let mut raw = key.text().to_vec();
            raw.push(0);
            if let StrKey::File(source, _) = &key {
                let source = *source;
                // A text the file kept inside another (a shared tail, an empty string on a terminator) points into
                // that text's new copy again.
                let host = placed.iter().rev().find(|h| {
                    h.0 <= source
                        && source + raw.len() <= h.0 + h.1.len()
                        && h.1[source - h.0..source - h.0 + raw.len()] == raw[..]
                });
                if let Some(h) = host {
                    offsets.insert(key.clone(), h.2 + source - h.0);
                    continue;
                }
                // No earlier than it sat: text a tool orphaned in the table keeps its room.
                let len = self.p.len();
                self.at_least(len, source);
                placed.push((source, raw.clone(), self.p.len()));
            }
            offsets.insert(key, self.p.len());
            self.p.extend_from_slice(&raw);
        }
        for ((slot, _, _), key) in pending.iter().zip(keys) {
            let at = offsets[&key.unwrap()];
            self.u32(*slot, at as i64 - *slot as i64);
            self.u32(*slot + 4, 0);
        }
        // Some tools pad the string table (to keep every file one size); that tail stays too.
        let len = self.p.len();
        self.at_least(len, self.doc.data_end - self.doc.data_start);
        self.strings_pending = pending;
    }

    fn resolve_pointers(&mut self) -> Result<()> {
        for (slot, ptr) in self.pointers_pending.clone() {
            match ptr {
                Pointer::Internal(i) => {
                    let Some(&at) = self.instance_offsets.get(i) else {
                        return err("EBX internal pointer index is invalid");
                    };
                    self.u32(slot, at as i64 - slot as i64);
                    self.pointer_offsets.push(slot);
                }
                Pointer::External(i) => {
                    if i >= self.doc.imports.len() {
                        return err("EBX import pointer index is invalid");
                    }
                    self.u32(slot, ((i as i64) << 1) | 1);
                    self.import_offsets.push(slot);
                }
                Pointer::Null => {}
            }
            self.u32(slot + 4, 0);
        }
        Ok(())
    }

    fn table(out: &mut Vec<u8>, values: &[usize]) {
        let values = sorted_unique(values);
        out.extend_from_slice(&(values.len() as u32).to_le_bytes());
        for v in values {
            out.extend_from_slice(&(v as u32).to_le_bytes());
        }
    }

    fn write_fixup(&self, string_offset: usize) -> Vec<u8> {
        let doc = self.doc;
        let mut out = doc.file_guid.0.to_vec();
        out.extend_from_slice(&(doc.fixup_type_guids.len() as u32).to_le_bytes());
        for g in &doc.fixup_type_guids {
            out.extend_from_slice(&g.0);
        }
        out.extend_from_slice(&(doc.fixup_type_signatures.len() as u32).to_le_bytes());
        for s in &doc.fixup_type_signatures {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out.extend_from_slice(&(doc.instances.iter().filter(|i| i.exported).count() as u32).to_le_bytes());
        Self::table(&mut out, &self.instance_offsets);
        Self::table(&mut out, &self.pointer_offsets);
        Self::table(&mut out, &self.resource_offsets);
        out.extend_from_slice(&(doc.imports.len() as u32).to_le_bytes());
        for (f, c) in &doc.imports {
            out.extend_from_slice(&f.0);
            out.extend_from_slice(&c.0);
        }
        Self::table(&mut out, &self.import_offsets);
        Self::table(&mut out, &self.type_info_offsets);
        for v in [self.arrays_offset, self.boxed_offset, string_offset] {
            out.extend_from_slice(&(v as u32).to_le_bytes());
        }
        let mut old_slots = doc.boxed_value_slots.clone();
        old_slots.sort_unstable();
        let mut new_slots: Vec<usize> = self.boxes_pending.iter().map(|(s, _)| *s).collect();
        new_slots.sort_unstable();
        let mut trailing = Vec::new();
        for old in &doc.boxed_reference_offsets {
            if let Some(index) = old_slots.iter().position(|s| s == old) {
                if index < new_slots.len() {
                    trailing.push(new_slots[index]);
                }
            }
        }
        if doc.has_boxed_table || !trailing.is_empty() {
            Self::table(&mut out, &trailing);
        }
        out
    }

    fn write_extra(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.arrays_out.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.boxed_out.len() as u32).to_le_bytes());
        for list in [&self.arrays_out, &self.boxed_out] {
            let mut sorted: Vec<&ArrayRecord> = list.iter().collect();
            sorted.sort_by_key(|a| a.offset);
            for v in sorted {
                out.extend_from_slice(&(v.offset as u32).to_le_bytes());
                out.extend_from_slice(&v.count.to_le_bytes());
                out.extend_from_slice(&v.hash.to_le_bytes());
                out.extend_from_slice(&v.encoded_type.to_le_bytes());
                out.extend_from_slice(&v.class_ref.to_le_bytes());
            }
        }
        out
    }

    fn write_riff(&self, payload: &[u8], fixup: &[u8], extra: &[u8]) -> Vec<u8> {
        let doc = self.doc;
        let refl: &[u8; 4] = if doc.hashed_reflection_names { b"RFL2" } else { b"REFL" };
        let body_of = |cid: &[u8; 4]| -> Option<&[u8]> {
            match cid {
                b"EBXD" => Some(payload),
                b"EFIX" => Some(fixup),
                b"EBXX" => Some(extra),
                c if c == refl => Some(&doc.reflection_data),
                _ => None,
            }
        };
        let mut order: Vec<[u8; 4]> = doc.chunk_order.iter().filter(|c| body_of(c).is_some()).copied().collect();
        for c in [b"EBXD", b"EFIX", b"EBXX", b"RFL2", b"REFL"] {
            if body_of(c).is_some() && !order.contains(c) {
                order.push(*c);
            }
        }
        let mut out = Vec::with_capacity(payload.len() + fixup.len() + extra.len() + doc.reflection_data.len() + 64);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&[0u8; 4]);
        out.extend_from_slice(if doc.serialized_layout { b"EBXS" } else { b"EBX\x00" });
        for cid in &order {
            let body = body_of(cid).unwrap();
            // the data starts 16-aligned (12 zero bytes when EBXD comes first)
            let pad = if cid == b"EBXD" { (16 - (out.len() + 8) % 16) % 16 } else { 0 };
            out.extend_from_slice(cid);
            out.extend_from_slice(&((pad + body.len()) as u32).to_le_bytes());
            out.extend(std::iter::repeat_n(0u8, pad));
            out.extend_from_slice(body);
            if (pad + body.len()) & 1 != 0 {
                out.push(0);
            }
        }
        let size = (out.len() - 8) as u32;
        out[4..8].copy_from_slice(&size.to_le_bytes());
        out
    }
}

fn collect_hashes(obj: &Object, known: &mut HashMap<(usize, usize), u32>) {
    for fv in &obj.fields {
        if let Some(h) = fv.value.array_hash {
            known.entry((obj.descriptor, fv.descriptor)).or_insert(h);
        }
        collect_value(&fv.value, known);
    }
}

fn collect_value(v: &Value, known: &mut HashMap<(usize, usize), u32>) {
    match &v.data {
        Data::Object(o) => collect_hashes(o, known),
        Data::Array(items) => {
            for e in items {
                collect_value(e, known);
            }
        }
        _ => {}
    }
}

/// The bytes of an integer field (ebx.py `_number`): bools and floats convert as Python's `int()` does; a value
/// outside the field's width is an error.
fn int_bytes(t: u16, data: &Data) -> Result<Vec<u8>> {
    let v: i128 = match *data {
        Data::None => 0,
        Data::Bool(b) => b as i128,
        Data::Int(v) => v as i128,
        Data::UInt(v) => v as i128,
        Data::F32(v) if v.is_finite() => v.trunc() as i128,
        Data::F64(v) if v.is_finite() => v.trunc() as i128,
        _ => return err(format!("EBX integer field cannot hold {data:?}")),
    };
    let (lo, hi, n): (i128, i128, usize) = match t {
        INT8 => (i8::MIN as i128, i8::MAX as i128, 1),
        UINT8 => (0, u8::MAX as i128, 1),
        INT16 => (i16::MIN as i128, i16::MAX as i128, 2),
        UINT16 => (0, u16::MAX as i128, 2),
        INT32 | ENUM => (i32::MIN as i128, i32::MAX as i128, 4),
        UINT32 => (0, u32::MAX as i128, 4),
        INT64 => (i64::MIN as i128, i64::MAX as i128, 8),
        _ => (0, u64::MAX as i128, 8),
    };
    if v < lo || v > hi {
        return err(format!("EBX integer field exceeds its native width: {v}"));
    }
    Ok(v.to_le_bytes()[..n].to_vec())
}
