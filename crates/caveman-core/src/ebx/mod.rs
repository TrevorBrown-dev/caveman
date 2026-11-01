//! EBX documents (RIFF EBX/EBXS with EBXD, EFIX, EBXX and REFL/RFL2 chunks): read, edit, write.
//!
//! A port of `tools/re/ebx.py` (itself a port of ReSkate's `Engine/Resource/ebx_document.cpp` and
//! `ebx_writer.cpp`, GPL-3.0). [`read_document`] gives a [`Document`]; [`write_document`] turns it back into
//! bytes, byte for byte as read when nothing changed (`preserve_layout`), or packed tight
//! ([`write_document_with`] with `preserve_layout = false`, what Caveman's build uses for new assets).

mod read;
mod write;

use std::collections::HashMap;
use std::fmt;

pub use read::{read_document, read_file_guid};
pub use write::{write_document, write_document_with};

// ---------------------------------------------------------------- field types and categories (ebx.py constants)

/// Field types: `(encoded >> 4) & 0x1F` of a type or field descriptor's encoded type.
pub const INHERITED: u16 = 0;
pub const DBOBJECT: u16 = 1;
pub const STRUCTURE: u16 = 2;
pub const POINTER: u16 = 3;
pub const ARRAY: u16 = 4;
pub const FIXED_ARRAY: u16 = 5;
pub const STRING: u16 = 6;
pub const CSTRING: u16 = 7;
pub const ENUM: u16 = 8;
pub const FILEREF: u16 = 9;
pub const BOOL: u16 = 10;
pub const INT8: u16 = 11;
pub const UINT8: u16 = 12;
pub const INT16: u16 = 13;
pub const UINT16: u16 = 14;
pub const INT32: u16 = 15;
pub const UINT32: u16 = 16;
pub const UINT64: u16 = 17;
pub const INT64: u16 = 18;
pub const FLOAT32: u16 = 19;
pub const FLOAT64: u16 = 20;
pub const GUID: u16 = 21;
pub const SHA1: u16 = 22;
pub const RESOURCEREF: u16 = 23;
pub const FUNCTION: u16 = 24;
pub const TYPEREF: u16 = 25;
pub const BOXEDVALUEREF: u16 = 26;
pub const INTERFACE: u16 = 27;
pub const DELEGATE: u16 = 28;

/// Field categories: `encoded & 0x0F`.
pub const CAT_NONE: u16 = 0;
pub const CAT_POINTER: u16 = 1;
pub const CAT_STRUCTURE: u16 = 2;
pub const CAT_PRIMITIVE: u16 = 3;
pub const CAT_ARRAY: u16 = 4;

/// The name ebx.py's `TYPE_NAMES` gives a field type.
pub fn type_name_of(t: u16) -> &'static str {
    const NAMES: [&str; 29] = [
        "inherited",
        "dbobject",
        "structure",
        "pointer",
        "array",
        "fixed_array",
        "string",
        "cstring",
        "enum",
        "fileref",
        "bool",
        "int8",
        "uint8",
        "int16",
        "uint16",
        "int32",
        "uint32",
        "uint64",
        "int64",
        "float32",
        "float64",
        "guid",
        "sha1",
        "resourceref",
        "function",
        "typeref",
        "boxedvalueref",
        "interface",
        "delegate",
    ];
    NAMES.get(t as usize).copied().unwrap_or("?")
}

/// Bytes a value of a field type takes (ebx.py `SCALAR_SIZE.get(t, 8)`).
pub(crate) fn scalar_size(t: u16) -> usize {
    match t {
        BOOL | INT8 | UINT8 => 1,
        INT16 | UINT16 => 2,
        INT32 | UINT32 | FLOAT32 | ENUM => 4,
        INT64 | UINT64 | FLOAT64 | RESOURCEREF | POINTER | CSTRING | FILEREF | TYPEREF | DELEGATE => 8,
        GUID => 16,
        SHA1 => 20,
        STRING => 32,
        BOXEDVALUEREF => 16,
        _ => 8,
    }
}

/// ebx.py `_scalar_alignment`: a GUID is four-byte aligned in the game's own arrays (ReSkate's writer assumes 8).
pub(crate) fn scalar_alignment(t: u16) -> usize {
    if t == GUID {
        return 4;
    }
    scalar_size(t).clamp(1, 8)
}

/// ebx.py `align_up`.
pub fn align_up(value: usize, alignment: usize) -> usize {
    if alignment <= 1 {
        value
    } else {
        value.div_ceil(alignment) * alignment
    }
}

// ---------------------------------------------------------------- errors

/// A malformed document, or a value that cannot be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn err<T>(message: impl Into<String>) -> Result<T> {
    Err(Error(message.into()))
}

// ---------------------------------------------------------------- value types

/// 16 bytes in the native (little-endian) GUID layout; `Display` gives the usual text form (ebx.py `Guid`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct Guid(pub [u8; 16]);

impl Guid {
    /// From the usual text form (`2259d419-387f-b1ba-30fc-c5140100b012`), as `uuid.UUID(text).bytes_le`.
    pub fn parse(text: &str) -> Result<Guid> {
        let hex: String = text.trim().trim_matches(|c| c == '{' || c == '}').chars().filter(|c| *c != '-').collect();
        if hex.len() != 32 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return err(format!("not a GUID: {text}"));
        }
        let mut be = [0u8; 16];
        for (i, b) in be.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| Error(format!("not a GUID: {text}")))?;
        }
        Ok(Guid::from_be(be))
    }

    /// From the big-endian (RFC 4122, `uuid.UUID.bytes`) layout.
    pub fn from_be(be: [u8; 16]) -> Guid {
        let mut le = be;
        le[0..4].reverse();
        le[4..6].reverse();
        le[6..8].reverse();
        Guid(le)
    }

    /// The big-endian (RFC 4122) layout.
    pub fn to_be(self) -> [u8; 16] {
        let mut be = self.0;
        be[0..4].reverse();
        be[4..6].reverse();
        be[6..8].reverse();
        be
    }

    /// A random version-4 GUID (ebx.py `Guid.new`: `uuid.uuid4().bytes_le`).
    pub fn new_random() -> Guid {
        let mut be = random_bytes::<16>();
        be[6] = (be[6] & 0x0F) | 0x40;
        be[8] = (be[8] & 0x3F) | 0x80;
        Guid::from_be(be)
    }

    pub fn is_zero(&self) -> bool {
        self.0 == [0u8; 16]
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = self.to_be();
        write!(
            f,
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
        )
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Guid({self})")
    }
}

/// Cryptographically random bytes from the operating system.
pub(crate) fn random_bytes<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    #[cfg(windows)]
    {
        #[link(name = "bcrypt")]
        extern "system" {
            fn BCryptGenRandom(algorithm: *mut std::ffi::c_void, buffer: *mut u8, count: u32, flags: u32) -> i32;
        }
        const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 2;
        // SAFETY: the buffer is N writable bytes; a null algorithm handle is valid with the system-preferred flag.
        let status = unsafe { BCryptGenRandom(std::ptr::null_mut(), out.as_mut_ptr(), N as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
        assert!(status >= 0, "BCryptGenRandom failed: {status:#x}");
    }
    #[cfg(not(windows))]
    {
        use std::io::Read;
        std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut out)).expect("no system randomness");
    }
    out
}

/// A resource reference. `listed`: whether the file's fixup table named this slot (the game leaves null references
/// out; ReSkate's writer lists every one). `None` for a new reference, which is listed (ebx.py `ResourceRef`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ResourceRef {
    pub id: u64,
    pub listed: Option<bool>,
}

impl ResourceRef {
    pub fn new(id: u64) -> ResourceRef {
        ResourceRef { id, listed: None }
    }
}

/// A pointer: null, an instance of this document, or an import (ebx.py `Pointer`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Pointer {
    #[default]
    Null,
    /// An index into [`Document::instances`].
    Internal(usize),
    /// An index into [`Document::imports`].
    External(usize),
}

/// A type reference (ebx.py `TypeRef`): `encoded` is what the file holds; the rest is decoded from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct TypeRef {
    pub primitive: bool,
    pub primitive_type: u32,
    pub descriptor: i64,
    pub encoded: u32,
}

/// A boxed value reference (ebx.py `BoxedRef`): its type and where its bytes sat (EBXD offset), and the file's
/// displacement, kept for references whose target is not a boxed record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BoxedRef {
    pub encoded_type: u32,
    pub data_offset: i64,
    pub relative_offset: i64,
}

/// A value's data (ebx.py `Value.data`).
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Data {
    #[default]
    None,
    Bool(bool),
    /// INT8, INT16, INT32, INT64 and ENUM fields.
    Int(i64),
    /// UINT8, UINT16, UINT32 and UINT64 fields.
    UInt(u64),
    /// Exact bits are kept (NaN payloads, infinities), as ebx.py's `F32Bits`.
    F32(f32),
    F64(f64),
    Guid(Guid),
    Sha1([u8; 20]),
    /// STRING, CSTRING and FILEREF text, as raw bytes (ebx.py decodes with `surrogateescape`, which keeps them).
    Str(Vec<u8>),
    ResourceRef(ResourceRef),
    Pointer(Pointer),
    TypeRef(TypeRef),
    Boxed(BoxedRef),
    Object(Object),
    Array(Vec<Value>),
}

impl Data {
    /// Text from a string (UTF-8; invalid bytes replaced).
    pub fn as_str(&self) -> Option<String> {
        match self {
            Data::Str(b) => Some(String::from_utf8_lossy(b).into_owned()),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Data::Int(v) => Some(v),
            Data::UInt(v) => i64::try_from(v).ok(),
            Data::Bool(b) => Some(b as i64),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            Data::UInt(v) => Some(v),
            Data::Int(v) => u64::try_from(v).ok(),
            Data::Bool(b) => Some(b as u64),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Data::F32(v) => Some(v as f64),
            Data::F64(v) => Some(v),
            Data::Int(v) => Some(v as f64),
            Data::UInt(v) => Some(v as f64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Data::Bool(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_guid(&self) -> Option<Guid> {
        match *self {
            Data::Guid(g) => Some(g),
            _ => None,
        }
    }

    pub fn as_pointer(&self) -> Option<Pointer> {
        match *self {
            Data::Pointer(p) => Some(p),
            _ => None,
        }
    }

    pub fn as_resource_ref(&self) -> Option<ResourceRef> {
        match *self {
            Data::ResourceRef(r) => Some(r),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&Object> {
        match self {
            Data::Object(o) => Some(o),
            _ => None,
        }
    }

    pub fn as_object_mut(&mut self) -> Option<&mut Object> {
        match self {
            Data::Object(o) => Some(o),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Data::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_array_mut(&mut self) -> Option<&mut Vec<Value>> {
        match self {
            Data::Array(a) => Some(a),
            _ => None,
        }
    }

    /// Python truthiness, as ebx.py's writer tests a BOOL field's value.
    pub(crate) fn truthy(&self) -> bool {
        match self {
            Data::None => false,
            Data::Bool(b) => *b,
            Data::Int(v) => *v != 0,
            Data::UInt(v) => *v != 0,
            Data::F32(v) => *v != 0.0,
            Data::F64(v) => *v != 0.0,
            Data::Str(s) => !s.is_empty(),
            Data::Array(a) => !a.is_empty(),
            _ => true,
        }
    }
}

impl From<&str> for Data {
    fn from(text: &str) -> Data {
        Data::Str(text.as_bytes().to_vec())
    }
}

impl From<String> for Data {
    fn from(text: String) -> Data {
        Data::Str(text.into_bytes())
    }
}

impl From<bool> for Data {
    fn from(v: bool) -> Data {
        Data::Bool(v)
    }
}

impl From<f32> for Data {
    fn from(v: f32) -> Data {
        Data::F32(v)
    }
}

impl From<Guid> for Data {
    fn from(v: Guid) -> Data {
        Data::Guid(v)
    }
}

impl From<Pointer> for Data {
    fn from(v: Pointer) -> Data {
        Data::Pointer(v)
    }
}

impl From<ResourceRef> for Data {
    fn from(v: ResourceRef) -> Data {
        Data::ResourceRef(v)
    }
}

/// A value: its data, an array's EBXX record hash, and where an array or string read from a file sat in its EBXD
/// (the writer keeps them in that order, so an unchanged document writes back byte for byte) (ebx.py `Value`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Value {
    pub data: Data,
    pub array_hash: Option<u32>,
    pub source_offset: Option<usize>,
}

impl Value {
    pub fn new(data: impl Into<Data>) -> Value {
        Value { data: data.into(), array_hash: None, source_offset: None }
    }
}

impl From<Data> for Value {
    fn from(data: Data) -> Value {
        Value { data, array_hash: None, source_offset: None }
    }
}

/// A field of an object: its descriptor index, name and value (ebx.py `FieldValue`).
#[derive(Clone, Debug, PartialEq)]
pub struct FieldValue {
    pub descriptor: usize,
    pub name: String,
    pub value: Value,
}

/// An object (an instance or a struct): its type descriptor and fields, inherited ones included (ebx.py `Object`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Object {
    pub descriptor: usize,
    pub fields: Vec<FieldValue>,
}

impl Object {
    /// The field named `name`, case-insensitive (ebx.py `Object.find`).
    pub fn find(&self, name: &str) -> Option<&FieldValue> {
        self.fields.iter().find(|f| f.name.eq_ignore_ascii_case(name))
    }

    pub fn find_mut(&mut self, name: &str) -> Option<&mut FieldValue> {
        self.fields.iter_mut().find(|f| f.name.eq_ignore_ascii_case(name))
    }

    /// A field's value by path (`Name`, `ItemData.Thumbnail`, `Values[2].Key`) (ebx.py `_walk`).
    pub fn value(&self, path: &str) -> Result<&Value> {
        walk(self, path)
    }

    pub fn value_mut(&mut self, path: &str) -> Result<&mut Value> {
        walk_mut(self, path)
    }

    /// A field's data by path.
    pub fn get(&self, path: &str) -> Result<&Data> {
        Ok(&walk(self, path)?.data)
    }

    /// Sets a field's data by path. The data must be the kind the field holds.
    pub fn set(&mut self, path: &str, data: impl Into<Data>) -> Result<()> {
        walk_mut(self, path)?.data = data.into();
        Ok(())
    }
}

fn split_part(part: &str) -> Result<(&str, Vec<usize>)> {
    let mut pieces = part.split('[');
    let name = pieces.next().unwrap_or("");
    let mut indices = Vec::new();
    for p in pieces {
        indices.push(p.parse::<usize>().map_err(|_| Error(format!("bad index in path part {part:?}")))?);
    }
    Ok((name, indices))
}

fn walk<'a>(obj: &'a Object, path: &str) -> Result<&'a Value> {
    let cleaned = path.replace(']', "");
    let parts: Vec<&str> = cleaned.split('.').collect();
    let mut obj = obj;
    for (k, part) in parts.iter().enumerate() {
        let (name, indices) = split_part(part)?;
        let field = obj.find(name).ok_or_else(|| Error(format!("no field {name:?} in {path:?}")))?;
        let mut value = &field.value;
        for i in indices {
            value = value.data.as_array().and_then(|a| a.get(i)).ok_or_else(|| Error(format!("no element {i} in {path:?}")))?;
        }
        if k + 1 == parts.len() {
            return Ok(value);
        }
        obj = value.data.as_object().ok_or_else(|| Error(format!("{name:?} is not an object in {path:?}")))?;
    }
    err(format!("empty path {path:?}"))
}

fn walk_mut<'a>(obj: &'a mut Object, path: &str) -> Result<&'a mut Value> {
    let cleaned = path.replace(']', "");
    let parts: Vec<String> = cleaned.split('.').map(str::to_string).collect();
    walk_mut_parts(obj, &parts, path)
}

fn walk_mut_parts<'a>(obj: &'a mut Object, parts: &[String], path: &str) -> Result<&'a mut Value> {
    let (name, indices) = split_part(&parts[0])?;
    let field = obj.find_mut(name).ok_or_else(|| Error(format!("no field {name:?} in {path:?}")))?;
    let mut value = &mut field.value;
    for i in indices {
        value = value.data.as_array_mut().and_then(|a| a.get_mut(i)).ok_or_else(|| Error(format!("no element {i} in {path:?}")))?;
    }
    if parts.len() == 1 {
        return Ok(value);
    }
    let inner = value.data.as_object_mut().ok_or_else(|| Error(format!("{name:?} is not an object in {path:?}")))?;
    walk_mut_parts(inner, &parts[1..], path)
}

// ---------------------------------------------------------------- descriptors and records

/// A type descriptor of the reflection table (ebx.py `TypeDescriptor`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TypeDescriptor {
    pub name: String,
    pub name_hash: u32,
    pub field_index: i32,
    pub field_count: u16,
    pub encoded_type: u16,
    pub size: u16,
    pub alignment: u16,
    pub guid: Guid,
    pub signature: u32,
}

impl TypeDescriptor {
    pub fn type_(&self) -> u16 {
        (self.encoded_type >> 4) & 0x1F
    }

    pub fn category(&self) -> u16 {
        self.encoded_type & 0x0F
    }
}

/// A field descriptor of the reflection table (ebx.py `FieldDescriptor`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FieldDescriptor {
    pub name: String,
    pub name_hash: u32,
    pub encoded_type: u16,
    pub class_ref: u16,
    pub data_offset: u32,
}

impl FieldDescriptor {
    pub fn type_(&self) -> u16 {
        (self.encoded_type >> 4) & 0x1F
    }

    pub fn category(&self) -> u16 {
        self.encoded_type & 0x0F
    }
}

/// An instance (ebx.py `InstanceRecord`): its fixup type slot, type descriptor (None when the reflection has no
/// match), EBXD offset, whether it is exported (has a GUID), its object and the bytes it was read from.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct InstanceRecord {
    pub fixup_type: u16,
    pub descriptor: Option<usize>,
    pub data_offset: usize,
    pub exported: bool,
    pub instance_guid: Guid,
    pub object: Option<Object>,
    pub raw_image: Vec<u8>,
}

/// An EBXX array record (ebx.py `ArrayRecord`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ArrayRecord {
    pub offset: usize,
    pub count: u32,
    pub hash: u32,
    pub encoded_type: u16,
    pub class_ref: u16,
}

/// An EBXX boxed value record with its bytes (ebx.py `BoxedValueRecord`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BoxedValueRecord {
    pub offset: usize,
    pub count: u32,
    pub hash: u32,
    pub encoded_type: u16,
    pub class_ref: u16,
    pub raw_bytes: Vec<u8>,
}

/// An EBX document (ebx.py `Document`).
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub file_guid: Guid,
    pub root_type: String,
    pub data_start: usize,
    pub data_end: usize,
    pub arrays_offset: usize,
    pub boxed_values_offset: usize,
    pub strings_offset: usize,
    /// EBXS (true) or EBX.
    pub serialized_layout: bool,
    /// RFL2 (true) or REFL.
    pub hashed_reflection_names: bool,
    pub reflection_data: Vec<u8>,
    pub fixup_type_guids: Vec<Guid>,
    pub fixup_type_signatures: Vec<u32>,
    pub boxed_reference_offsets: Vec<usize>,
    pub types: Vec<TypeDescriptor>,
    pub fields: Vec<FieldDescriptor>,
    pub instances: Vec<InstanceRecord>,
    /// (file GUID, instance GUID).
    pub imports: Vec<(Guid, Guid)>,
    pub arrays: Vec<ArrayRecord>,
    pub boxed_values: Vec<BoxedValueRecord>,
    pub pointer_offsets: Vec<usize>,
    pub resource_ref_offsets: Vec<usize>,
    pub import_offsets: Vec<usize>,
    pub type_info_offsets: Vec<usize>,
    pub boxed_value_slots: Vec<usize>,
    /// The EBXD bytes as read (offsets as in the fixup tables).
    pub payload: Vec<u8>,
    /// (hash, count, first).
    pub reflection_groups: Vec<(u32, u32, u32)>,
    /// Whether EFIX ended with a boxed reference table (ebx.py `_has_boxed_table`; true for a new document).
    pub has_boxed_table: bool,
    /// The file's RIFF chunk order (ebx.py `_chunk_order`).
    pub chunk_order: Vec<[u8; 4]>,
}

impl Default for Document {
    fn default() -> Document {
        Document {
            file_guid: Guid::default(),
            root_type: String::new(),
            data_start: 0,
            data_end: 0,
            arrays_offset: 0,
            boxed_values_offset: 0,
            strings_offset: 0,
            serialized_layout: false,
            hashed_reflection_names: false,
            reflection_data: Vec::new(),
            fixup_type_guids: Vec::new(),
            fixup_type_signatures: Vec::new(),
            boxed_reference_offsets: Vec::new(),
            types: Vec::new(),
            fields: Vec::new(),
            instances: Vec::new(),
            imports: Vec::new(),
            arrays: Vec::new(),
            boxed_values: Vec::new(),
            pointer_offsets: Vec::new(),
            resource_ref_offsets: Vec::new(),
            import_offsets: Vec::new(),
            type_info_offsets: Vec::new(),
            boxed_value_slots: Vec::new(),
            payload: Vec::new(),
            reflection_groups: Vec::new(),
            has_boxed_table: true,
            chunk_order: Vec::new(),
        }
    }
}

impl Document {
    /// The root instance (the first).
    pub fn root(&self) -> Option<&InstanceRecord> {
        self.instances.first()
    }

    /// A type descriptor's name, or `?` (ebx.py `type_name`).
    pub fn type_name(&self, descriptor: usize) -> &str {
        self.types.get(descriptor).map(|t| t.name.as_str()).unwrap_or("?")
    }

    /// The type name of an instance.
    pub fn instance_type(&self, index: usize) -> &str {
        match self.instances.get(index).and_then(|i| i.descriptor) {
            Some(d) => self.type_name(d),
            None => "?",
        }
    }

    /// Indices of the instances whose type is `type_name`, case-insensitive (ebx.py `instances_of`).
    pub fn instances_of(&self, type_name: &str) -> Vec<usize> {
        (0..self.instances.len()).filter(|&i| self.instance_type(i).eq_ignore_ascii_case(type_name)).collect()
    }

    /// An instance's object.
    pub fn object(&self, instance: usize) -> Result<&Object> {
        self.instances.get(instance).and_then(|i| i.object.as_ref()).ok_or_else(|| Error(format!("instance {instance} has no object")))
    }

    pub fn object_mut(&mut self, instance: usize) -> Result<&mut Object> {
        self.instances.get_mut(instance).and_then(|i| i.object.as_mut()).ok_or_else(|| Error(format!("instance {instance} has no object")))
    }

    /// A field's data by path from an instance's object (ebx.py `get`).
    pub fn get(&self, path: &str, instance: usize) -> Result<&Data> {
        self.object(instance)?.get(path)
    }

    /// Sets a field's data by path (ebx.py `set`). `data` must be the kind the field holds.
    pub fn set(&mut self, path: &str, data: impl Into<Data>, instance: usize) -> Result<()> {
        self.object_mut(instance)?.set(path, data)
    }

    /// Every string value: (instance index, path, text) (ebx.py `strings`).
    pub fn strings(&self) -> Vec<(usize, String, String)> {
        let mut out = Vec::new();
        for (i, inst) in self.instances.iter().enumerate() {
            if let Some(obj) = &inst.object {
                object_strings(obj, "", &mut |path, text| out.push((i, path, text)));
            }
        }
        out
    }

    /// The imports as (file GUID text, instance GUID text) (ebx.py `import_list`).
    pub fn import_list(&self) -> Vec<(String, String)> {
        self.imports.iter().map(|(f, c)| (f.to_string(), c.to_string())).collect()
    }

    /// Sets the asset's own name (the root object's Name field) (ebx.py `rename`).
    pub fn rename(&mut self, name: &str) -> Result<()> {
        let root = self.object_mut(0)?;
        let field = root.find_mut("Name").ok_or_else(|| Error("the root object has no Name field".into()))?;
        field.value.data = Data::from(name);
        Ok(())
    }

    pub fn set_file_guid(&mut self, guid: Guid) {
        self.file_guid = guid;
    }

    pub fn set_instance_guid(&mut self, index: usize, guid: Guid) {
        self.instances[index].instance_guid = guid;
    }

    /// (file GUID, root instance GUID): how another document imports this one.
    pub fn reference(&self) -> (Guid, Guid) {
        (self.file_guid, self.instances.first().map(|i| i.instance_guid).unwrap_or_default())
    }

    /// Replaces import GUIDs by {old GUID (file or instance): new GUID} (ebx.py `remap_imports`).
    pub fn remap_imports(&mut self, mapping: &HashMap<Guid, Guid>) {
        for (f, c) in self.imports.iter_mut() {
            if let Some(n) = mapping.get(f) {
                *f = *n;
            }
            if let Some(n) = mapping.get(c) {
                *c = *n;
            }
        }
    }

    /// Adds an import and returns a pointer to it.
    pub fn add_import(&mut self, reference: (Guid, Guid)) -> Pointer {
        self.imports.push(reference);
        Pointer::External(self.imports.len() - 1)
    }

    /// Adds a copy of instance `index` with instance GUID `guid` and returns its index. An exported copy goes after
    /// the last exported instance (the writer needs exported instances first), and internal pointers to the instances
    /// after it move along; an internal copy goes last. Pointers inside the copy still point where the original's do.
    /// The copy's arrays and strings carry no source offsets, so the writer lays them out as new ones.
    pub fn clone_instance(&mut self, index: usize, guid: Guid) -> Result<usize> {
        let mut copy = self.instances.get(index).cloned().ok_or_else(|| Error(format!("no instance {index}")))?;
        copy.instance_guid = guid;
        copy.data_offset = 0;
        if let Some(obj) = copy.object.as_mut() {
            clear_source_offsets(obj);
        }
        let at = if copy.exported { self.instances.iter().position(|i| !i.exported).unwrap_or(self.instances.len()) } else { self.instances.len() };
        if at < self.instances.len() {
            for inst in self.instances.iter_mut() {
                if let Some(obj) = inst.object.as_mut() {
                    shift_internal_pointers(obj, at);
                }
            }
            if let Some(obj) = copy.object.as_mut() {
                shift_internal_pointers(obj, at);
            }
        }
        self.instances.insert(at, copy);
        Ok(at)
    }

    /// The fixup type slot of a type, added when the document has none for it (ebx.py `ensure_fixup_type`).
    pub fn ensure_fixup_type(&mut self, descriptor: usize) -> usize {
        let (guid, signature) = (self.types[descriptor].guid, self.types[descriptor].signature);
        let n = self.fixup_type_guids.len().min(self.fixup_type_signatures.len());
        for i in 0..n {
            if self.fixup_type_guids[i] == guid && self.fixup_type_signatures[i] == signature {
                return i;
            }
        }
        self.fixup_type_guids.insert(n, guid);
        self.fixup_type_signatures.push(signature);
        n
    }
}

fn object_strings(obj: &Object, prefix: &str, out: &mut dyn FnMut(String, String)) {
    for f in &obj.fields {
        value_strings(&f.value, format!("{prefix}{}", f.name), out);
    }
}

fn value_strings(v: &Value, path: String, out: &mut dyn FnMut(String, String)) {
    match &v.data {
        Data::Str(s) => out(path, String::from_utf8_lossy(s).into_owned()),
        Data::Object(o) => object_strings(o, &format!("{path}."), out),
        Data::Array(a) => {
            for (i, e) in a.iter().enumerate() {
                value_strings(e, format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}

/// Every value of an object, nested ones included.
fn values_mut(obj: &mut Object, f: &mut dyn FnMut(&mut Value)) {
    for field in obj.fields.iter_mut() {
        value_mut(&mut field.value, f);
    }
}

fn value_mut(v: &mut Value, f: &mut dyn FnMut(&mut Value)) {
    f(v);
    match &mut v.data {
        Data::Object(o) => values_mut(o, f),
        Data::Array(a) => {
            for e in a.iter_mut() {
                value_mut(e, f);
            }
        }
        _ => {}
    }
}

/// Forgets where an object's arrays and strings sat in the file it was read from (Document::clone_instance).
fn clear_source_offsets(obj: &mut Object) {
    values_mut(obj, &mut |v| v.source_offset = None);
}

/// Moves internal pointers at or after instance `at` one along (an instance was inserted at `at`).
fn shift_internal_pointers(obj: &mut Object, at: usize) {
    values_mut(obj, &mut |v| {
        if let Data::Pointer(Pointer::Internal(i)) = &mut v.data {
            if *i >= at {
                *i += 1;
            }
        }
    });
}
