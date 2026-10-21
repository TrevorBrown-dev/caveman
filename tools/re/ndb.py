"""Frostbite's native DB container (layout.toc, InitFS, bundle chunk metadata), ported from ReSkate's
Engine/Vfs/native_db (GPL-3.0). read() then write() gives back the same bytes."""
import struct


class Node:
    __slots__ = ('type', 'named', 'terminated', 'name', 'payload', 'children')

    def __init__(self, type=0, name='', payload=b'', children=None, named=True, terminated=False):
        self.type, self.name, self.payload = type, name, payload
        self.children = children if children is not None else []
        self.named, self.terminated = named, terminated

    def field(self, key):
        return next((c for c in self.children if c.named and c.name == key), None)

    @property
    def text(self):
        t = self.payload.decode('latin-1')
        return t[:-1] if t.endswith('\0') else t

    def value(self):
        """Python value: dict/list for containers, str, bytes, int or bool."""
        if self.type == 2:
            return {c.name: c.value() for c in self.children}
        if self.type == 1:
            return [c.value() for c in self.children]
        if self.type == 7:
            return self.text
        if self.type == 6:
            return self.payload[0] != 0
        if self.type in (8, 11):
            return struct.unpack('<i' if self.type == 8 else '<f', self.payload)[0]
        if self.type in (9, 12):
            return struct.unpack('<q' if self.type == 9 else '<d', self.payload)[0]
        return self.payload


def read(data, pos=0, depth=0):
    """(node, end position)"""
    tag = data[pos]; pos += 1
    node = Node(tag & 31)
    if not node.type:
        return node, pos
    node.named = (tag & 128) == 0
    if node.named:
        end = data.index(0, pos)
        node.name = data[pos:end].decode('latin-1'); pos = end + 1
    if node.type in (1, 2):
        size, pos = _var(data, pos)
        end = pos + size
        while pos < end:
            child, pos = read(data, pos, depth + 1)
            if not child.type:
                node.terminated = True
                break
            node.children.append(child)
        assert pos == end, 'DB container length mismatch'
    elif node.type in (7, 19):
        size, pos = _var(data, pos)
        node.payload = bytes(data[pos:pos + size]); pos += size
    else:
        size = {6: 1, 8: 4, 11: 4, 9: 8, 12: 8, 15: 16, 16: 20}[node.type]
        node.payload = bytes(data[pos:pos + size]); pos += size
    return node, pos


def _var(data, pos):
    value = shift = 0
    while True:
        b = data[pos]; pos += 1
        value |= (b & 127) << shift
        if not b & 128:
            return value, pos
        shift += 7


def _put_var(out, value):
    while True:
        b = value & 127
        value >>= 7
        if not value:
            out.append(b)
            return
        out.append(b | 128)


def write(node):
    out = bytearray()
    _emit(node, out)
    return bytes(out)


def _emit(node, out):
    if not node.type:
        out.append(0)
        return
    out.append(node.type | (0 if node.named else 128))
    if node.named:
        out += node.name.encode('latin-1') + b'\0'
    if node.type in (1, 2):
        body = bytearray()
        for c in node.children:
            _emit(c, body)
        if node.terminated:
            body.append(0)
        _put_var(out, len(body))
        out += body
        return
    if node.type in (7, 19):
        _put_var(out, len(node.payload))
    out += node.payload


def dump(node, indent=0):
    pad = '  ' * indent
    label = node.name if node.named else '-'
    if node.type in (1, 2):
        print(f'{pad}{label} ({"list" if node.type == 1 else "object"}, {len(node.children)})')
        for c in node.children:
            dump(c, indent + 1)
    else:
        v = node.value()
        if isinstance(v, bytes):
            v = v.hex()
        print(f'{pad}{label} = {v!r} (type {node.type})')
