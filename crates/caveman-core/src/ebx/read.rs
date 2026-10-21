//! Reading EBX (ebx.py `read_document`, `read_file_guid`, `_ValueReader` and the chunk readers).

use std::collections::{HashMap, HashSet};

use super::*;

const RIFF: &[u8; 4] = b"RIFF";
const EBX: &[u8; 4] = b"EBX\x00";
const EBXS: &[u8; 4] = b"EBXS";

/// A byte cursor (ebx.py `_Reader`).
struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Reader<'a> {
        Reader { b, pos: 0 }
    }

    fn seek(&mut self, pos: usize) -> Result<()> {
        if pos > self.b.len() {
            return err("reader seek is out of range");
        }
        self.pos = pos;
        Ok(())
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos + n > self.b.len() {
            return err("reader ran past the end");
        }
        let out = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().unwrap())
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    fn guid(&mut self) -> Result<Guid> {
        Ok(Guid(self.array()?))
    }

    /// A count field (i32): negative counts read as zero items, as Python's `range` does.
    fn count(&mut self) -> Result<usize> {
        Ok(self.i32()?.max(0) as usize)
    }
}

fn u16_at(b: &[u8], at: usize) -> Result<u16> {
    b.get(at..at + 2).map(|s| u16::from_le_bytes(s.try_into().unwrap())).ok_or_else(|| Error("read past the end".into()))
}

fn i32_at(b: &[u8], at: usize) -> Result<i32> {
    b.get(at..at + 4).map(|s| i32::from_le_bytes(s.try_into().unwrap())).ok_or_else(|| Error("read past the end".into()))
}

fn find_nul(b: &[u8], from: usize) -> Option<usize> {
    b.get(from..).and_then(|s| s.iter().position(|&c| c == 0)).map(|p| from + p)
}

/// A slice clamped to the buffer, as Python slicing is.
fn clamped(b: &[u8], start: usize, end: usize) -> &[u8] {
    let end = end.min(b.len());
    let start = start.min(end);
    &b[start..end]
}

type Chunk = ([u8; 4], usize, usize);

fn chunks(r: &mut Reader) -> Result<(Vec<Chunk>, [u8; 4])> {
    if r.take(4)? != RIFF {
        return err("EBX is not a RIFF document");
    }
    let riff_end = r.u32()? as usize + 8;
    if riff_end != r.b.len() {
        return err("RIFF EBX size does not match its stream");
    }
    let form: [u8; 4] = r.array()?;
    if &form != EBX && &form != EBXS {
        return err("RIFF document is not EBX or EBXS");
    }
    let mut out = Vec::new();
    while r.pos < riff_end {
        let cid: [u8; 4] = r.array()?;
        let size = r.u32()? as usize;
        let begin = r.pos;
        if size > riff_end - begin {
            return err("RIFF chunk exceeds its document");
        }
        out.push((cid, begin, begin + size));
        r.seek(begin + size + (size & 1))?;
    }
    Ok((out, form))
}

fn chunk(chunks: &[Chunk], cid: &[u8; 4], required: bool) -> Result<Option<Chunk>> {
    let found: Vec<&Chunk> = chunks.iter().filter(|c| &c.0 == cid).collect();
    if found.len() > 1 {
        return err(format!("duplicate RIFF {:?} chunk", String::from_utf8_lossy(cid)));
    }
    if found.is_empty() && required {
        return err(format!("missing RIFF {:?} chunk", String::from_utf8_lossy(cid)));
    }
    Ok(found.first().map(|c| **c))
}

fn check(doc: &Document, offset: usize, required: usize, label: &str) -> Result<()> {
    let size = doc.data_end - doc.data_start;
    if offset > size || required > size - offset {
        return err(format!("{label} offset exceeds EBXD"));
    }
    Ok(())
}

fn read_offsets(r: &mut Reader, doc: &Document, label: &str, width: usize) -> Result<Vec<usize>> {
    let n = r.count()?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let offset = r.u32()? as usize;
        check(doc, offset, width, label)?;
        out.push(offset);
    }
    Ok(out)
}

/// ebx.py `_read_fixup`.
fn read_fixup(r: &mut Reader, efix: Chunk, doc: &mut Document, data: &[u8]) -> Result<()> {
    r.seek(efix.1)?;
    doc.file_guid = r.guid()?;
    let n = r.count()?;
    let guids: Vec<Guid> = (0..n).map(|_| r.guid()).collect::<Result<_>>()?;
    let n = r.count()?;
    if n > guids.len() {
        return err("EFIX has more signatures than types");
    }
    let sigs: Vec<u32> = (0..n).map(|_| r.u32()).collect::<Result<_>>()?;
    let exported = r.i32()?;
    let count = r.i32()?;
    if exported > count {
        return err("EFIX exported instance count is invalid");
    }
    for index in 0..count.max(0) {
        let offset = r.u32()? as usize;
        check(doc, offset, 2, "EFIX instance")?;
        let class_ref = u16_at(data, doc.data_start + offset)?;
        if class_ref as usize >= guids.len() {
            return err("EFIX instance type is invalid");
        }
        doc.instances.push(InstanceRecord {
            fixup_type: class_ref,
            descriptor: None,
            data_offset: offset,
            exported: index < exported,
            ..Default::default()
        });
    }
    doc.fixup_type_guids = guids;
    doc.fixup_type_signatures = sigs;
    doc.pointer_offsets = read_offsets(r, doc, "EFIX pointer", 8)?;
    doc.resource_ref_offsets = read_offsets(r, doc, "EFIX resource reference", 8)?;
    let n = r.count()?;
    doc.imports = (0..n).map(|_| Ok((r.guid()?, r.guid()?))).collect::<Result<_>>()?;
    doc.import_offsets = read_offsets(r, doc, "EFIX import relocation", 8)?;
    doc.type_info_offsets = read_offsets(r, doc, "EFIX type-info reference", 8)?;
    doc.arrays_offset = r.u32()? as usize;
    doc.boxed_values_offset = r.u32()? as usize;
    doc.strings_offset = r.u32()? as usize;
    for (o, label) in [(doc.arrays_offset, "array"), (doc.boxed_values_offset, "boxed-value"), (doc.strings_offset, "string")] {
        check(doc, o, 0, &format!("EFIX {label} section"))?;
    }
    doc.has_boxed_table = r.pos < efix.2;
    if doc.has_boxed_table {
        doc.boxed_reference_offsets = read_offsets(r, doc, "EFIX boxed reference", 16)?;
    }
    if r.pos != efix.2 {
        return err("unexpected bytes at the end of EFIX");
    }
    Ok(())
}

/// ebx.py `_read_extra`.
fn read_extra(r: &mut Reader, ebxx: Chunk, doc: &mut Document) -> Result<()> {
    r.seek(ebxx.1)?;
    let na = r.count()?;
    let nb = r.count()?;
    for k in 0..na + nb {
        let offset = r.u32()? as usize;
        let count = r.i32()? as u32;
        let hash = r.u32()?;
        let encoded_type = r.u16()?;
        let class_ref = r.u16()?;
        if k < na {
            check(doc, offset, 0, "EBXX array")?;
            doc.arrays.push(ArrayRecord { offset, count, hash, encoded_type, class_ref });
        } else {
            check(doc, offset, 0, "EBXX boxed value")?;
            doc.boxed_values.push(BoxedValueRecord { offset, count, hash, encoded_type, class_ref, raw_bytes: Vec::new() });
        }
    }
    if r.pos != ebxx.2 {
        return err("unexpected bytes at the end of EBXX");
    }
    Ok(())
}

fn string_at(b: &[u8], offset: usize, end: usize) -> Result<String> {
    let stop = find_nul(b, offset).ok_or_else(|| Error("reflection string is unterminated".into()))?;
    if stop >= end {
        return err("reflection string is unterminated");
    }
    Ok(String::from_utf8_lossy(&b[offset..stop]).into_owned())
}

/// ebx.py `_read_reflection`. Returns the (GUID, signature) of each type.
fn read_reflection(r: &mut Reader, rc: Chunk, hashed: bool, doc: &mut Document) -> Result<Vec<(Guid, u32)>> {
    r.seek(rc.1)?;
    let n = r.count()?;
    let ids: Vec<(Guid, u32)> = (0..n).map(|_| Ok((r.guid()?, r.u32()?))).collect::<Result<_>>()?;
    let mut type_names = Vec::new();
    for index in 0..r.count()? {
        let name_hash = r.u32()?;
        let field_index = r.i32()?;
        let field_count = r.u16()?;
        let et = r.u16()?;
        let size = r.u16()?;
        let alignment = r.u16()?;
        let (guid, signature) = ids.get(index).copied().unwrap_or_default();
        type_names.push(name_hash);
        doc.types.push(TypeDescriptor {
            name: String::new(),
            name_hash,
            field_index,
            field_count,
            encoded_type: et >> 1,
            size,
            alignment,
            guid,
            signature,
        });
    }
    let mut field_names = Vec::new();
    for _ in 0..r.count()? {
        let name_hash = r.u32()?;
        let data_offset = r.u32()?;
        let et = r.u16()?;
        let class_ref = r.u16()?;
        field_names.push(name_hash);
        doc.fields.push(FieldDescriptor { name: String::new(), name_hash, encoded_type: et >> 1, class_ref, data_offset });
    }
    for t in &doc.types {
        if t.field_index < 0
            || t.field_index as usize > doc.fields.len()
            || t.field_count as usize > doc.fields.len() - t.field_index as usize
        {
            return err("reflection type field range is invalid");
        }
    }
    let n = r.count()?;
    doc.reflection_groups = (0..n).map(|_| Ok((r.u32()?, r.u32()?, r.u32()?))).collect::<Result<_>>()?;
    let mappings = r.i32()?;
    r.seek((r.pos as i64 + 8 * mappings as i64) as usize)?;
    if hashed {
        let mut name_offsets = HashMap::new();
        for _ in 0..r.count()? {
            let h = r.u32()?;
            let o = r.u32()?;
            name_offsets.insert(h, o as usize);
        }
        let strings = r.pos;
        let lookup = |h: u32| -> Result<usize> {
            name_offsets.get(&h).copied().ok_or_else(|| Error(format!("reflection name {h:#x} is missing")))
        };
        for (t, h) in doc.types.iter_mut().zip(&type_names) {
            t.name = string_at(r.b, strings + lookup(*h)?, rc.2)?;
        }
        for (f, h) in doc.fields.iter_mut().zip(&field_names) {
            f.name = string_at(r.b, strings + lookup(*h)?, rc.2)?;
        }
    } else {
        let strings = r.pos;
        for (t, h) in doc.types.iter_mut().zip(&type_names) {
            t.name = string_at(r.b, strings + *h as usize, rc.2)?;
        }
        for (f, h) in doc.fields.iter_mut().zip(&field_names) {
            f.name = string_at(r.b, strings + *h as usize, rc.2)?;
        }
    }
    Ok(ids)
}

/// ebx.py `_ValueReader`.
struct ValueReader<'a> {
    b: &'a [u8],
    pos: usize,
    data_start: usize,
    data_end: usize,
    types: &'a [TypeDescriptor],
    fields: &'a [FieldDescriptor],
    imports: usize,
    array_hashes: HashMap<usize, u32>,
    instance_at: HashMap<usize, usize>,
    resource_slots: HashSet<usize>,
    boxed_value_slots: Vec<usize>,
    depth: usize,
}

impl<'a> ValueReader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos + n > self.b.len() {
            return err("reader ran past the end");
        }
        let out = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    fn arr<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().unwrap())
    }

    fn seek(&mut self, pos: usize) -> Result<()> {
        if pos > self.b.len() {
            return err("reader seek is out of range");
        }
        self.pos = pos;
        Ok(())
    }

    fn read_object(&mut self, descriptor: usize, start: usize) -> Result<Object> {
        self.depth += 1;
        if self.depth > 128 {
            return err("EBX types nest too deeply");
        }
        let t = &self.types[descriptor];
        if start + t.size as usize > self.data_end {
            return err("EBX object exceeds EBXD");
        }
        let mut obj = Object { descriptor, fields: Vec::new() };
        self.read_fields(&mut obj, descriptor, start)?;
        self.depth -= 1;
        Ok(obj)
    }

    fn read_fields(&mut self, obj: &mut Object, type_index: usize, start: usize) -> Result<()> {
        let t = &self.types[type_index];
        for k in 0..t.field_count as usize {
            let di = t.field_index as usize + k;
            let f = &self.fields[di];
            if f.type_() == INHERITED {
                self.depth += 1;
                self.read_fields(obj, f.class_ref as usize, start)?;
                self.depth -= 1;
                continue;
            }
            let pos = start + f.data_offset as usize;
            if pos > self.data_end {
                return err("EBX field offset exceeds EBXD");
            }
            let mut value = self.read_field(f, pos)?;
            if f.category() == CAT_ARRAY {
                let target = (pos as i64 + i32_at(self.b, pos)? as i64 - self.data_start as i64) as usize;
                value.source_offset = Some(target);
                if matches!(&value.data, Data::Array(a) if !a.is_empty()) {
                    if let Some(h) = self.array_hashes.get(&target) {
                        value.array_hash = Some(*h);
                    }
                }
            }
            obj.fields.push(FieldValue { descriptor: di, name: f.name.clone(), value });
        }
        Ok(())
    }

    fn read_field(&mut self, f: &FieldDescriptor, pos: usize) -> Result<Value> {
        if f.category() == CAT_ARRAY {
            let disp = i32_at(self.b, pos)? as i64;
            let target = pos as i64 + disp;
            if target < self.data_start as i64 + 4 || target > self.data_end as i64 {
                return err("EBX array displacement exceeds EBXD");
            }
            let target = target as usize;
            let count = i32_at(self.b, target - 4)?;
            if count < 0 || count as usize > self.data_end - target {
                return err("EBX array count is invalid");
            }
            self.seek(target)?;
            let ft = f.type_();
            let mut values = Vec::with_capacity(count as usize);
            for _ in 0..count {
                let element = self.pos;
                values.push(self.read_value(ft, f.class_ref as usize, element)?);
                if ft == STRUCTURE {
                    self.seek(element + self.types[f.class_ref as usize].size as usize)?;
                } else if ft == POINTER || ft == CSTRING {
                    let aligned = align_up(self.pos, 8);
                    self.seek(aligned)?;
                }
            }
            return Ok(Value::from(Data::Array(values)));
        }
        self.seek(pos)?;
        self.read_value(f.type_(), f.class_ref as usize, pos)
    }

    fn read_value(&mut self, t: u16, class_ref: usize, pos: usize) -> Result<Value> {
        let data = match t {
            BOOL => Data::Bool(self.arr::<1>()?[0] != 0),
            INT8 => Data::Int(i8::from_le_bytes(self.arr()?) as i64),
            UINT8 => Data::UInt(u8::from_le_bytes(self.arr()?) as u64),
            INT16 => Data::Int(i16::from_le_bytes(self.arr()?) as i64),
            UINT16 => Data::UInt(u16::from_le_bytes(self.arr()?) as u64),
            INT32 | ENUM => Data::Int(i32::from_le_bytes(self.arr()?) as i64),
            UINT32 => Data::UInt(u32::from_le_bytes(self.arr()?) as u64),
            INT64 => Data::Int(i64::from_le_bytes(self.arr()?)),
            UINT64 => Data::UInt(u64::from_le_bytes(self.arr()?)),
            FLOAT32 => Data::F32(f32::from_bits(u32::from_le_bytes(self.arr()?))),
            FLOAT64 => Data::F64(f64::from_bits(u64::from_le_bytes(self.arr()?))),
            GUID => Data::Guid(Guid(self.arr()?)),
            SHA1 => Data::Sha1(self.arr()?),
            RESOURCEREF => {
                let id = u64::from_le_bytes(self.arr()?);
                let listed = self.resource_slots.contains(&(pos - self.data_start));
                Data::ResourceRef(ResourceRef { id, listed: Some(listed) })
            }
            STRING => {
                let raw = self.take(32)?;
                let end = raw.iter().position(|&c| c == 0).unwrap_or(32);
                Data::Str(raw[..end].to_vec())
            }
            CSTRING | FILEREF => {
                let disp = u32::from_le_bytes(self.arr()?);
                if t == FILEREF {
                    self.pos += 4;
                }
                let text = self.relative_string(pos, disp)?;
                return Ok(Value { data: Data::Str(text), array_hash: None, source_offset: self.string_source(pos, disp) });
            }
            POINTER => {
                let encoded = i32::from_le_bytes(self.arr()?);
                if encoded == 0 {
                    Data::Pointer(Pointer::Null)
                } else if encoded & 1 != 0 {
                    let index = encoded >> 1;
                    if index < 0 || index as usize >= self.imports {
                        return err("EBX pointer import is invalid");
                    }
                    Data::Pointer(Pointer::External(index as usize))
                } else {
                    let target = pos as i64 + encoded as i64 - self.data_start as i64;
                    match usize::try_from(target).ok().and_then(|t| self.instance_at.get(&t)) {
                        Some(&i) => Data::Pointer(Pointer::Internal(i)),
                        None => return err("EBX internal pointer target is invalid"),
                    }
                }
            }
            TYPEREF | DELEGATE => {
                let encoded = u32::from_le_bytes(self.arr()?);
                self.pos += 4;
                if encoded == 0 {
                    Data::TypeRef(TypeRef::default())
                } else if encoded & 0x8000_0000 != 0 {
                    Data::TypeRef(TypeRef {
                        primitive: true,
                        primitive_type: ((encoded & 0x7FFF_FFFF) >> 5) & 0x1F,
                        descriptor: -1,
                        encoded,
                    })
                } else {
                    Data::TypeRef(TypeRef { primitive: false, primitive_type: 0, descriptor: (encoded >> 2) as i64, encoded })
                }
            }
            BOXEDVALUEREF => {
                let encoded = u32::from_le_bytes(self.arr()?);
                self.pos += 4;
                let disp = i64::from_le_bytes(self.arr()?);
                let target = pos as i64 + 8 + disp - self.data_start as i64;
                self.boxed_value_slots.push(pos - self.data_start);
                Data::Boxed(BoxedRef { encoded_type: encoded, data_offset: target, relative_offset: disp })
            }
            STRUCTURE => Data::Object(self.read_object(class_ref, pos)?),
            _ => Data::None,
        };
        Ok(Value::from(data))
    }

    fn string_source(&self, base: usize, disp: u32) -> Option<usize> {
        if disp == 0xFFFF_FFFF {
            None
        } else {
            Some(base + disp as usize - self.data_start)
        }
    }

    fn relative_string(&self, base: usize, disp: u32) -> Result<Vec<u8>> {
        if disp == 0xFFFF_FFFF {
            return Ok(Vec::new());
        }
        let target = base + disp as usize;
        if target < self.data_start || target >= self.data_end {
            return err("EBX string displacement exceeds EBXD");
        }
        let end = find_nul(self.b, target).ok_or_else(|| Error("EBX string exceeds EBXD".into()))?;
        if end >= self.data_end {
            return err("EBX string exceeds EBXD");
        }
        Ok(self.b[target..end].to_vec())
    }
}

/// Reads an EBX document (ebx.py `read_document`).
pub fn read_document(data: &[u8]) -> Result<Document> {
    let mut r = Reader::new(data);
    let (chunks, form) = chunks(&mut r)?;
    let ebxd = chunk(&chunks, b"EBXD", true)?.unwrap();
    let efix = chunk(&chunks, b"EFIX", true)?.unwrap();
    let ebxx = chunk(&chunks, b"EBXX", true)?.unwrap();
    let refl = chunk(&chunks, b"REFL", false)?;
    let rfl2 = chunk(&chunks, b"RFL2", false)?;
    if refl.is_none() == rfl2.is_none() {
        return err("RIFF EBX must contain exactly one REFL or RFL2 chunk");
    }
    let mut doc = Document { serialized_layout: &form == EBXS, ..Default::default() };
    doc.data_start = align_up(ebxd.1, 16);
    doc.data_end = ebxd.2;
    if doc.data_start > doc.data_end {
        return err("EBXD alignment exceeds the chunk");
    }
    doc.chunk_order = chunks.iter().map(|c| c.0).collect();
    read_fixup(&mut r, efix, &mut doc, data)?;
    read_extra(&mut r, ebxx, &mut doc)?;
    let rc = rfl2.or(refl).unwrap();
    let ids = read_reflection(&mut r, rc, rfl2.is_some(), &mut doc)?;
    doc.hashed_reflection_names = rfl2.is_some();
    doc.reflection_data = data[rc.1..rc.2].to_vec();
    doc.payload = data[doc.data_start..doc.data_end].to_vec();
    let mut id_index: HashMap<(Guid, u32), usize> = HashMap::new();
    for (i, key) in ids.iter().enumerate() {
        id_index.entry(*key).or_insert(i);
    }
    for inst in doc.instances.iter_mut() {
        let ft = inst.fixup_type as usize;
        if ft < doc.fixup_type_guids.len() && ft < doc.fixup_type_signatures.len() {
            inst.descriptor = id_index.get(&(doc.fixup_type_guids[ft], doc.fixup_type_signatures[ft])).copied();
        }
    }
    if let Some(d) = doc.instances.first().and_then(|i| i.descriptor) {
        if d < doc.types.len() {
            doc.root_type = doc.types[d].name.clone();
        }
    }
    read_instances(data, &mut doc)?;
    doc.boxed_values.sort_by_key(|v| v.offset);
    for i in 0..doc.boxed_values.len() {
        let end = doc.boxed_values.get(i + 1).map(|v| v.offset).unwrap_or(doc.strings_offset);
        let offset = doc.boxed_values[i].offset;
        if end < offset {
            return err("EBXX boxed values are not ordered");
        }
        doc.boxed_values[i].raw_bytes = clamped(data, doc.data_start + offset, doc.data_start + end).to_vec();
    }
    Ok(doc)
}

/// ebx.py `_ValueReader.read_instances`.
fn read_instances(data: &[u8], doc: &mut Document) -> Result<()> {
    let mut vr = ValueReader {
        b: data,
        pos: 0,
        data_start: doc.data_start,
        data_end: doc.data_end,
        types: &doc.types,
        fields: &doc.fields,
        imports: doc.imports.len(),
        array_hashes: doc.arrays.iter().map(|a| (a.offset, a.hash)).collect(),
        instance_at: doc.instances.iter().enumerate().map(|(i, inst)| (inst.data_offset, i)).collect(),
        resource_slots: doc.resource_ref_offsets.iter().copied().collect(),
        boxed_value_slots: Vec::new(),
        depth: 0,
    };
    let mut results = Vec::with_capacity(doc.instances.len());
    for inst in &doc.instances {
        let Some(descriptor) = inst.descriptor.filter(|&d| d < vr.types.len()) else {
            results.push(None);
            continue;
        };
        let start = doc.data_start + inst.data_offset;
        let guid = if inst.exported {
            if start < 16 {
                return err("EBX exported instance has no room for its GUID");
            }
            Some(Guid(data[start - 16..start].try_into().unwrap()))
        } else {
            None
        };
        let size = vr.types[descriptor].size as usize;
        if start + size > doc.data_end {
            return err("EBX object exceeds EBXD");
        }
        let raw = data[start..start + size].to_vec();
        let object = vr.read_object(descriptor, start)?;
        results.push(Some((guid, raw, object)));
    }
    let slots = std::mem::take(&mut vr.boxed_value_slots);
    drop(vr);
    doc.boxed_value_slots.extend(slots);
    for (inst, result) in doc.instances.iter_mut().zip(results) {
        if let Some((guid, raw, object)) = result {
            if let Some(g) = guid {
                inst.instance_guid = g;
            }
            inst.raw_image = raw;
            inst.object = Some(object);
        }
    }
    Ok(())
}

/// The file GUID of an EBX document, without reading the rest (ebx.py `read_file_guid`).
pub fn read_file_guid(data: &[u8]) -> Result<Guid> {
    let mut r = Reader::new(data);
    let (chunks, _) = chunks(&mut r)?;
    let efix = chunk(&chunks, b"EFIX", true)?.unwrap();
    data.get(efix.1..efix.1 + 16)
        .map(|b| Guid(b.try_into().unwrap()))
        .ok_or_else(|| Error("EFIX is too short".into()))
}
