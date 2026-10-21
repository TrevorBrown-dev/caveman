"""Read-only Frostbite readers for ReSkate mods, ported from ReSkate's Engine/Resource (GPL-3.0)."""
import ctypes
import glob
import os
import struct

ENVELOPE = 0x22C
SALT = 0x7065636E
BUNDLE_MAGIC = 0xED1CEDB8
TEXTURE_TYPE = 0x6BDE20BA


def be32(b, o):
    return struct.unpack_from('>I', b, o)[0]


def bei32(b, o):
    return struct.unpack_from('>i', b, o)[0]


def guid_str(raw):
    """Frostbite little-endian GUID bytes as the usual text form."""
    import uuid
    return str(uuid.UUID(bytes_le=bytes(raw)))


# ---------------------------------------------------------------- Oodle
_oodle = None


def oodle(game_root):
    global _oodle
    if _oodle is None:
        dll = sorted(glob.glob(os.path.join(game_root, 'oo2core_*_win64.dll')))[-1]
        _oodle = ctypes.CDLL(dll)
        _oodle.OodleLZ_Decompress.restype = ctypes.c_int64
        _oodle.OodleLZ_Decompress.argtypes = [ctypes.c_void_p, ctypes.c_int64, ctypes.c_void_p, ctypes.c_int64,
                                              ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_void_p,
                                              ctypes.c_int64, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p,
                                              ctypes.c_int64, ctypes.c_int]
    return _oodle


def decode_cas(data, game_root):
    out = bytearray()
    at = 0
    blocks = []
    while at < len(data):
        decoded = be32(data, at)
        comp = data[at + 4] | (data[at + 5] << 8)
        enc = struct.unpack_from('>H', data, at + 6)[0]
        flags = comp >> 8
        enc += (flags & 0x0F) << 16
        decoded &= 0xFFFFFF
        comp &= 0x7F
        payload = data[at + 8: at + 8 + enc]
        at += 8 + enc
        blocks.append(comp)
        if comp == 0:
            out += payload
        elif comp in (0x11, 0x15, 0x19):
            o = oodle(game_root)
            buf = ctypes.create_string_buffer(decoded + 0x40000)
            src = ctypes.create_string_buffer(bytes(payload), len(payload))
            n = o.OodleLZ_Decompress(src, len(payload), buf, decoded, 1, 0, 0, None, 0, None, None, None, 0, 3)
            if n != decoded:
                raise RuntimeError('oodle block failed')
            out += buf.raw[:decoded]
        elif comp == 0x0F:
            import zstandard  # not expected
            out += zstandard.ZstdDecompressor().decompress(payload, max_output_size=decoded)
        else:
            raise RuntimeError(f'compression {comp:#x}')
    return bytes(out), blocks


# ---------------------------------------------------------------- TOC
class Huffman:
    def __init__(self, d, names_off, names_count, table_off, table_count):
        self.words = [be32(d, names_off + 4 * i) for i in range(names_count)]
        nodes = []  # (value, left, right)
        by_value = {}
        left = None
        counter = 0
        self.root = None
        for i in range(table_count):
            v = be32(d, table_off + 4 * i)
            if v in by_value:
                node = by_value[v]
            else:
                node = len(nodes)
                nodes.append((v, None, None))
                by_value[v] = node
            if left is None:
                left = node
                continue
            parent = len(nodes)
            nodes.append((counter, left, node))
            by_value.setdefault(counter, parent)
            self.root = parent
            counter += 1
            left = None
        self.nodes = nodes

    def decode(self, bit):
        out = []
        total = len(self.words) * 32
        while bit < total:
            n = self.root
            while self.nodes[n][1] is not None:
                one = (self.words[bit >> 5] >> (bit & 31)) & 1
                n = self.nodes[n][2] if one else self.nodes[n][1]
                bit += 1
            ch = (~self.nodes[n][0]) & 0xFFFF
            if ch == 0:
                return ''.join(out)
            out.append(chr(ch))
        raise RuntimeError('unterminated name')


def read_toc(path):
    raw = open(path, 'rb').read()
    assert raw[:4] == b'\x00\xd1\xce\x01', 'bad TOC envelope'
    d = raw[ENVELOPE:]
    (bundle_hash_off, bundle_data_off, bundle_count, chunk_hash_off, chunk_guid_off, chunk_count,
     _a, _b, names_off, chunk_data_off, data_count, flags) = struct.unpack_from('>IIiIIiIIIIiI', d, 0)
    huff = None
    if flags & 4:
        names_count, table_count, table_off = struct.unpack_from('>III', d, 48)
        huff = Huffman(d, names_off, names_count, table_off, table_count)
    bundles = []
    for i in range(bundle_count):
        o = bundle_data_off + 16 * i
        name_off, size_flag = struct.unpack_from('>II', d, o)
        region_off = struct.unpack_from('>Q', d, o + 8)[0]
        size = size_flag & 0x3FFFFFFF
        load = size_flag >> 30
        if huff:
            name = huff.decode(name_off)
        else:
            e = d.index(b'\x00', names_off + name_off)
            name = d[names_off + name_off:e].decode()
        bundles.append({'name': name, 'load': load, 'region': d[region_off:region_off + size] if load == 1 else None,
                        'region_off': region_off, 'size': size})
    chunks = []
    if chunk_count:
        words = [be32(d, chunk_data_off + 4 * i) for i in range(data_count)]
        for i in range(chunk_count):
            o = chunk_guid_off + 20 * i
            g = bytes(reversed(d[o:o + 16]))
            enc = bei32(d, o + 16)
            if enc == -1:
                chunks.append({'guid': guid_str(g), 'removed': True})
                continue
            w = enc & 0xFFFFFF
            ident = (words[w] << 32) | words[w + 1]
            chunks.append({'guid': guid_str(g), 'patch': bool(ident & (0xFF << 48)), 'install': (ident >> 16) & 0xFFFFFFFF,
                           'archive': ident & 0xFFFF, 'offset': words[w + 2], 'size': words[w + 3]})
    return {'flags': flags, 'bundles': bundles, 'chunks': chunks}


# ---------------------------------------------------------------- bundles
def read_bundle_region(r):
    (meta_off, meta_size, loc_off, total, data_off, res_off, chunk_off, _x, total2) = struct.unpack_from('>iiIiIIIIi', r, 0)
    assert total == total2
    inline = r[meta_off:meta_off + meta_size] if meta_size else None
    files = []
    at = data_off
    cur = None
    for i in range(total):
        flag = r[loc_off + i]
        if flag == 1:
            e = be32(r, at); at += 4
            cur = (bool(e & 0x00FF0000), (e >> 8) & 0xFF, e & 0xFF)
        elif flag != 0:
            v = struct.unpack_from('>Q', r, at)[0]; at += 8
            cur = (bool(v & (0xFF << 48)), (v >> 16) & 0xFFFFFFFF, v & 0xFFFF)
        off, size = struct.unpack_from('>II', r, at); at += 8
        files.append({'patch': cur[0], 'install': cur[1], 'archive': cur[2], 'offset': off, 'size': size})
    return inline, files


def read_binary_bundle(d):
    declared = be32(d, 0)
    assert declared == len(d) - 4, 'bundle size mismatch'
    magic = be32(d, 4) ^ SALT
    end = '>' if magic == BUNDLE_MAGIC else '<'
    if end == '<':
        assert struct.unpack_from('<I', d, 4)[0] ^ SALT == BUNDLE_MAGIC
    u = lambda o: struct.unpack_from(end + 'I', d, o)[0]
    total, ne, nr, nc, strings, meta_off, meta_size = (u(8 + 4 * i) for i in range(7))
    strings += 4
    at = 36
    hashes = [d[at + 20 * i: at + 20 * i + 20] for i in range(total)]
    at += 20 * total

    def name(off):
        e = d.index(b'\x00', strings + off)
        return d[strings + off:e].decode()
    ebx = []
    for i in range(ne):
        ebx.append({'name': name(u(at)), 'size': u(at + 4), 'sha1': hashes[i].hex()}); at += 8
    res = []
    for i in range(nr):
        res.append({'name': name(u(at)), 'size': u(at + 4), 'sha1': hashes[ne + i].hex()}); at += 8
    for r in res:
        r['type'] = u(at); at += 4
    for r in res:
        r['meta'] = d[at:at + 16]; at += 16
    for r in res:
        r['rid'] = struct.unpack_from(end + 'Q', d, at)[0]; at += 8
    chunks = []
    for i in range(nc):
        g = d[at:at + 16]
        if end == '>':
            g = bytes(reversed(g))
        lo, ls = struct.unpack_from(end + 'II', d, at + 16)
        chunks.append({'guid': guid_str(g), 'logical_offset': lo, 'logical_size': ls, 'sha1': hashes[ne + nr + i].hex()})
        at += 24
    meta = d[4 + meta_off: 4 + meta_off + meta_size] if meta_size else b''
    return {'endian': end, 'ebx': ebx, 'res': res, 'chunks': chunks, 'chunk_meta': meta}


class Layer:
    """A mod folder (or the game's Data folder) whose cas archives are resolved by archive number."""

    def __init__(self, root, game_root):
        self.root = root
        self.game_root = game_root
        self.cas = {}
        for p in glob.glob(os.path.join(root, 'Win32', '**', 'cas_*.cas'), recursive=True):
            rel = os.path.relpath(os.path.dirname(p), os.path.join(root, 'Win32')).replace('\\', '/').lower()
            idx = int(os.path.basename(p)[4:-4])
            self.cas.setdefault(idx, []).append((rel, p))

    def read_raw(self, f):
        cands = self.cas.get(f['archive'], [])
        if len(cands) != 1:
            raise RuntimeError(f"archive {f['archive']} ambiguous or missing in {self.root}: {cands}")
        with open(cands[0][1], 'rb') as h:
            h.seek(f['offset'])
            return h.read(f['size'])

    def bundle(self, region):
        inline, files = read_bundle_region(region)
        if inline is not None:
            return read_binary_bundle(inline), files
        raw = self.read_raw(files[0])
        try:
            m = read_binary_bundle(raw)
        except Exception:
            m = read_binary_bundle(decode_cas(raw, self.game_root)[0])
        return m, files[1:]


def texture_header(payload, meta):
    version = struct.unpack_from('<I', meta, 0)[0]
    ext = len(payload) == 180
    o = 8
    ttype, fmt = struct.unpack_from('<Ii', payload, o); o += 8
    if version >= 12:
        o += 4
    flags, w, h, depth, slices = struct.unpack_from('<HHHHH', payload, o); o += 10
    mips, first = payload[o], payload[o + 1]; o += 2
    o += 8 if ext else 4
    chunk = guid_str(payload[o:o + 16]); o += 16
    mip_sizes = struct.unpack_from('<15I', payload, o); o += 60
    chunk_size = struct.unpack_from('<I', payload, o)[0]
    return {'version': version, 'len': len(payload), 'type': ttype, 'format': fmt, 'flags': flags, 'w': w, 'h': h,
            'depth': depth, 'slices': slices, 'mips': mips, 'first_mip': first, 'chunk': chunk,
            'mip_sizes': mip_sizes[:mips], 'chunk_size': chunk_size}
