//! Native DB: chunk metadata, layout.toc, InitFS (ndb.py).
//!
//! Frostbite's native DB is a tree of tagged values: a tag byte (type in the low 5 bits, bit 7 set for an unnamed
//! value), a NUL-terminated latin-1 name for named values, then the payload: a varint-sized list (type 1) or object
//! (type 2) of children, a varint-sized string (7) or blob (19), or a fixed-size scalar (6 bool, 8 i32, 9 i64,
//! 11 f32, 12 f64, 15 GUID, 16 SHA-1). A container may end with a 0 tag inside its size. `read` then `write` gives
//! back the same bytes.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NdbError {
    /// The data ends inside a value.
    Truncated(usize),
    /// A scalar type with no known size.
    UnknownType(u8, usize),
    /// A container whose children do not end exactly at its size (`DB container length mismatch`).
    LengthMismatch(usize),
}

impl fmt::Display for NdbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NdbError::Truncated(at) => write!(f, "native DB truncated at {at:#x}"),
            NdbError::UnknownType(t, at) => write!(f, "native DB type {t} at {at:#x}"),
            NdbError::LengthMismatch(at) => write!(f, "DB container length mismatch at {at:#x}"),
        }
    }
}

impl std::error::Error for NdbError {}

/// Value types (the low 5 bits of a tag).
pub mod types {
    pub const END: u8 = 0;
    pub const LIST: u8 = 1;
    pub const OBJECT: u8 = 2;
    pub const BOOL: u8 = 6;
    pub const STRING: u8 = 7;
    pub const I32: u8 = 8;
    pub const I64: u8 = 9;
    pub const F32: u8 = 11;
    pub const F64: u8 = 12;
    pub const GUID: u8 = 15;
    pub const SHA1: u8 = 16;
    pub const BLOB: u8 = 19;
}

/// One value of the tree (`ndb.Node`).
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// The type (`types::*`); 0 is the end marker.
    pub kind: u8,
    /// Tag bits 5 and 6 as read (always 0 in the game's data; kept so writing gives back the same bytes).
    pub extra: u8,
    pub named: bool,
    /// The container ended with an explicit 0 tag inside its size.
    pub terminated: bool,
    /// The name (latin-1 bytes) of a named value.
    pub name: Vec<u8>,
    /// A scalar's, string's or blob's bytes (a string's include its trailing NUL).
    pub payload: Vec<u8>,
    pub children: Vec<Node>,
}

/// A value's plain Rust form (`ndb.Node.value`).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Object(Vec<(String, Value)>),
    List(Vec<Value>),
    Str(String),
    Bool(bool),
    I32(i32),
    I64(i64),
    F32(f32),
    F64(f64),
    Bytes(Vec<u8>),
}

fn scalar_size(kind: u8) -> Option<usize> {
    Some(match kind {
        types::BOOL => 1,
        types::I32 | types::F32 => 4,
        types::I64 | types::F64 => 8,
        types::GUID => 16,
        types::SHA1 => 20,
        _ => return None,
    })
}

fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

impl Node {
    /// A new named value.
    pub fn named(kind: u8, name: &str, payload: Vec<u8>) -> Node {
        Node { kind, extra: 0, named: true, terminated: false, name: name.chars().map(|c| c as u32 as u8).collect(), payload, children: Vec::new() }
    }

    /// The name as text (latin-1).
    pub fn name(&self) -> String {
        latin1(&self.name)
    }

    /// The first named child called `key` (`ndb.Node.field`).
    pub fn field(&self, key: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.named && c.name.iter().copied().eq(key.chars().map(|c| c as u32 as u8)))
    }

    /// `field`, mutable.
    pub fn field_mut(&mut self, key: &str) -> Option<&mut Node> {
        self.children.iter_mut().find(|c| c.named && c.name.iter().copied().eq(key.chars().map(|c| c as u32 as u8)))
    }

    /// A string's text without its trailing NUL (`ndb.Node.text`).
    pub fn text(&self) -> String {
        let t = self.payload.strip_suffix(&[0]).unwrap_or(&self.payload);
        latin1(t)
    }

    /// The payload as a little-endian i32 (type 8), if it is one.
    pub fn as_i32(&self) -> Option<i32> {
        (self.kind == types::I32).then(|| i32::from_le_bytes(self.payload[..4].try_into().expect("4")))
    }

    /// The payload as a little-endian i64 (type 9), if it is one.
    pub fn as_i64(&self) -> Option<i64> {
        (self.kind == types::I64).then(|| i64::from_le_bytes(self.payload[..8].try_into().expect("8")))
    }

    /// The payload as a little-endian u64 of any 8-byte scalar (chunk metadata `h64` is stored as type 9).
    pub fn as_u64(&self) -> Option<u64> {
        (self.payload.len() == 8).then(|| u64::from_le_bytes(self.payload[..8].try_into().expect("8")))
    }

    /// The value in plain form (`ndb.Node.value`).
    pub fn value(&self) -> Value {
        match self.kind {
            types::OBJECT => Value::Object(self.children.iter().map(|c| (c.name(), c.value())).collect()),
            types::LIST => Value::List(self.children.iter().map(|c| c.value()).collect()),
            types::STRING => Value::Str(self.text()),
            types::BOOL => Value::Bool(self.payload.first().copied().unwrap_or(0) != 0),
            types::I32 => Value::I32(i32::from_le_bytes(self.payload[..4].try_into().expect("4"))),
            types::F32 => Value::F32(f32::from_le_bytes(self.payload[..4].try_into().expect("4"))),
            types::I64 => Value::I64(i64::from_le_bytes(self.payload[..8].try_into().expect("8"))),
            types::F64 => Value::F64(f64::from_le_bytes(self.payload[..8].try_into().expect("8"))),
            _ => Value::Bytes(self.payload.clone()),
        }
    }
}

fn var(data: &[u8], mut pos: usize) -> Result<(usize, usize), NdbError> {
    let mut value: usize = 0;
    let mut shift = 0;
    loop {
        let b = *data.get(pos).ok_or(NdbError::Truncated(pos))?;
        pos += 1;
        if shift < usize::BITS {
            value |= ((b & 127) as usize) << shift;
        }
        if b & 128 == 0 {
            return Ok((value, pos));
        }
        shift += 7;
    }
}

fn put_var(out: &mut Vec<u8>, mut value: usize) {
    loop {
        let b = (value & 127) as u8;
        value >>= 7;
        if value == 0 {
            out.push(b);
            return;
        }
        out.push(b | 128);
    }
}

/// Reads one value at `pos` (`ndb.read`). Returns the value and the position after it.
pub fn read(data: &[u8], pos: usize) -> Result<(Node, usize), NdbError> {
    read_at(data, pos)
}

fn read_at(data: &[u8], mut pos: usize) -> Result<(Node, usize), NdbError> {
    let start = pos;
    let tag = *data.get(pos).ok_or(NdbError::Truncated(pos))?;
    pos += 1;
    let mut node = Node { kind: tag & 31, extra: tag & 0x60, named: false, terminated: false, name: Vec::new(), payload: Vec::new(), children: Vec::new() };
    if node.kind == types::END {
        return Ok((node, pos));
    }
    node.named = tag & 128 == 0;
    if node.named {
        let rest = data.get(pos..).ok_or(NdbError::Truncated(pos))?;
        let end = rest.iter().position(|&b| b == 0).ok_or(NdbError::Truncated(pos))?;
        node.name = rest[..end].to_vec();
        pos += end + 1;
    }
    match node.kind {
        types::LIST | types::OBJECT => {
            let (size, p) = var(data, pos)?;
            pos = p;
            let end = pos.checked_add(size).ok_or(NdbError::Truncated(pos))?;
            if end > data.len() {
                return Err(NdbError::Truncated(pos));
            }
            while pos < end {
                let (child, p) = read_at(data, pos)?;
                pos = p;
                if child.kind == types::END {
                    node.terminated = true;
                    break;
                }
                node.children.push(child);
            }
            if pos != end {
                return Err(NdbError::LengthMismatch(start));
            }
        }
        types::STRING | types::BLOB => {
            let (size, p) = var(data, pos)?;
            pos = p;
            node.payload = data.get(pos..pos + size).ok_or(NdbError::Truncated(pos))?.to_vec();
            pos += size;
        }
        k => {
            let size = scalar_size(k).ok_or(NdbError::UnknownType(k, start))?;
            node.payload = data.get(pos..pos + size).ok_or(NdbError::Truncated(pos))?.to_vec();
            pos += size;
        }
    }
    Ok((node, pos))
}

/// The bytes of a value (`ndb.write`).
pub fn write(node: &Node) -> Vec<u8> {
    let mut out = Vec::new();
    emit(node, &mut out);
    out
}

fn emit(node: &Node, out: &mut Vec<u8>) {
    if node.kind == types::END {
        out.push(0);
        return;
    }
    out.push(node.kind | node.extra | if node.named { 0 } else { 128 });
    if node.named {
        out.extend_from_slice(&node.name);
        out.push(0);
    }
    match node.kind {
        types::LIST | types::OBJECT => {
            let mut body = Vec::new();
            for c in &node.children {
                emit(c, &mut body);
            }
            if node.terminated {
                body.push(0);
            }
            put_var(out, body.len());
            out.extend_from_slice(&body);
        }
        types::STRING | types::BLOB => {
            put_var(out, node.payload.len());
            out.extend_from_slice(&node.payload);
        }
        _ => out.extend_from_slice(&node.payload),
    }
}
