"""Frostbite writers for ReSkate mods, ported from ReSkate's Engine/Resource (GPL-3.0): cas blocks,
bundle regions and patch TOCs."""
import struct
import uuid

ENVELOPE = 0x22C
FNV_PRIME = 0x01000193
BLOCK = 0x10000
MOD_INSTALL_CHUNK = 3162564249  # configurations/layout/initialinstallpackage
MOD_ARCHIVE = 1


def guid_bytes(text):
    """The in-memory (little-endian) form ReSkate's Guid holds."""
    return uuid.UUID(text).bytes_le


def encode_cas_raw(data):
    """Stored (uncompressed) 64 KiB blocks, as encode_cas writes them for CasCompression::raw.
    Only for small payloads: a chunk the game streams must be compressed like the game's own (see
    encode_cas), or it crashes reading it (a board equipped at spawn, 2026-10-20)."""
    out = bytearray()
    for at in range(0, max(1, len(data)), BLOCK):
        part = data[at:at + BLOCK]
        packed = ((len(part) & 0xFFFFFF) << 32) | (0x00 << 24) | (0x7 << 20) | len(part)
        out += struct.pack('>Q', packed) + part
    return bytes(out)


_oodle = None
KRAKEN = 0x11


def _oodle_lib(game_root):
    global _oodle
    if _oodle is None:
        import ctypes
        import glob
        import os
        dll = sorted(glob.glob(os.path.join(game_root, 'oo2core_*_win64.dll')))[-1]
        lib = ctypes.CDLL(dll)
        lib.OodleLZ_GetCompressedBufferSizeNeeded.restype = ctypes.c_int64
        lib.OodleLZ_GetCompressedBufferSizeNeeded.argtypes = [ctypes.c_int, ctypes.c_int64]
        lib.OodleLZ_Compress.restype = ctypes.c_int64
        lib.OodleLZ_Compress.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_int64, ctypes.c_void_p, ctypes.c_int,
                                         ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p, ctypes.c_int64]
        _oodle = (lib, ctypes)
    return _oodle


def encode_cas(data, game_root, level=4):
    """64 KiB blocks compressed with Oodle Kraken from the game's own oo2core, as ReSkate's encode_cas
    does by default: a block that does not shrink is stored raw, exactly as the game does."""
    lib, ctypes = _oodle_lib(game_root)
    out = bytearray()
    cap = lib.OodleLZ_GetCompressedBufferSizeNeeded(8, BLOCK)
    buf = ctypes.create_string_buffer(cap)
    for at in range(0, max(1, len(data)), BLOCK):
        part = bytes(data[at:at + BLOCK])
        src = ctypes.create_string_buffer(part, len(part))
        n = lib.OodleLZ_Compress(8, src, len(part), buf, level, None, None, None, None, 0) if part else 0
        if 0 < n < len(part):
            payload, comp = buf.raw[:n], KRAKEN
        else:
            payload, comp = part, 0x00
        if len(payload) > 0xFFFFF:
            raise OverflowError('encoded CAS block is too large')
        packed = ((len(part) & 0xFFFFFF) << 32) | (comp << 24) | (0x7 << 20) | len(payload)
        out += struct.pack('>Q', packed) + payload
    return bytes(out)


def block_offset(encoded, logical):
    """Byte offset in `encoded` of the block that decodes to `logical` onwards."""
    at = decoded = 0
    while at + 8 <= len(encoded):
        if decoded == logical:
            return at
        d = struct.unpack_from('>I', encoded, at)[0] & 0xFFFFFF
        comp = encoded[at + 4] | (encoded[at + 5] << 8)
        enc = struct.unpack_from('>H', encoded, at + 6)[0] + (((comp >> 8) & 0xF) << 16)
        at += 8 + enc
        decoded += d
    raise ValueError(f'no block starts at {logical:#x}')


def identifier(patch, install, archive):
    return (int(bool(patch)) << 48) | (install << 16) | archive


def write_bundle_region(files):
    table = bytearray()
    flags = bytearray()
    previous = None
    for f in files:
        loc = (f['patch'], f['install'], f['archive'])
        if previous == loc:
            flags.append(0)
        else:
            flags.append(0x80)
            table += struct.pack('>Q', identifier(*loc))
            previous = loc
        table += struct.pack('>II', f['offset'], f['size'])
    if flags:
        flags[0] |= 0x04
    data_offset = 0x24
    location_offset = data_offset + len(table)
    header = struct.pack('>iiIiIIIIi', 0, 0, location_offset, len(files), data_offset, data_offset, data_offset, 0,
                         len(files))
    return header + bytes(table) + bytes(flags)


SALT = 0x7065636E
BUNDLE_MAGIC = 0xED1CEDB8


def write_binary_bundle(m):
    """A bundle manifest from fb.read_binary_bundle's dict ('ebx', 'res', 'chunks', 'chunk_meta'), little
    endian as the game's own: the total size big endian, then the header, SHA-1s, entries, names."""
    e = '<'
    ebx, res, chunks = m['ebx'], m['res'], m['chunks']
    total = len(ebx) + len(res) + len(chunks)
    strings = bytearray()
    offsets = []
    for a in ebx + res:
        offsets.append(len(strings))
        strings += a['name'].encode() + b'\0'
    sha_start = 36
    ebx_start = sha_start + total * 20
    string_start = ebx_start + len(ebx) * 8 + len(res) * (8 + 4 + 16 + 8) + len(chunks) * 24
    meta = m.get('chunk_meta') or b''
    meta_start = string_start + len(strings)
    length = meta_start + len(meta)
    out = bytearray()
    out += struct.pack('>I', length - 4)
    out += struct.pack(e + 'IIIIIIII', BUNDLE_MAGIC ^ SALT, total, len(ebx), len(res), len(chunks),
                       string_start - 4, (meta_start - 4) if meta else 0, len(meta))
    for a in ebx + res + chunks:
        out += bytes.fromhex(a['sha1'])
    for i, a in enumerate(ebx):
        out += struct.pack(e + 'II', offsets[i], a['size'])
    for i, a in enumerate(res):
        out += struct.pack(e + 'II', offsets[len(ebx) + i], a['size'])
    for a in res:
        out += struct.pack(e + 'I', a['type'])
    for a in res:
        assert len(a['meta']) == 16
        out += a['meta']
    for a in res:
        out += struct.pack(e + 'Q', a['rid'])
    for c in chunks:
        out += guid_bytes(c['guid'])
        out += struct.pack(e + 'II', c['logical_offset'], c['logical_size'])
    out += strings
    out += meta
    assert len(out) == length
    return bytes(out)


def toc_hash(key, seed=0x811C9DC5):
    for b in key:
        signed = b - 256 if b >= 128 else b
        seed = ((seed * FNV_PRIME) & 0xFFFFFFFF) ^ (signed & 0xFFFFFFFF)
    return seed % FNV_PRIME


def perfect_hash(keys):
    size = len(keys)
    mapping = [-1] * size
    order = [None] * size
    if size == 0:
        return mapping, order
    if len(set(keys)) != size:
        raise ValueError('TOC perfect-hash keys must be unique')
    buckets = [[] for _ in range(size)]
    for i, k in enumerate(keys):
        buckets[toc_hash(k) % size].append(i)
    buckets.sort(key=lambda b: -len(b))  # stable, as std::stable_sort
    used = [False] * size
    bi = 0
    while bi < len(buckets) and len(buckets[bi]) > 1:
        bucket = buckets[bi]
        seed = 1
        while True:
            indices = []
            collision = False
            for member in bucket:
                idx = toc_hash(keys[member], seed) % size
                if used[idx] or idx in indices:
                    collision = True
                    break
                indices.append(idx)
            if not collision:
                break
            seed += 1
        mapping[toc_hash(keys[bucket[0]]) % size] = seed
        for j, member in enumerate(bucket):
            order[indices[j]] = member
            used[indices[j]] = True
        bi += 1
    free = 0
    while bi < len(buckets) and buckets[bi]:
        while used[free]:
            free = (free + 1) % size
        member = buckets[bi][0]
        mapping[toc_hash(keys[member]) % size] = -free - 1
        order[free] = member
        used[free] = True
        free = (free + 1) % size
        bi += 1
    return mapping, order


def align(v, a):
    return (v + a - 1) & ~(a - 1)


def write_patch_toc(bundles, chunks, flags=3):
    """bundles: [{'name', 'region', 'load'}]; chunks: [{'guid', 'patch', 'install', 'archive', 'offset', 'size'}]"""
    bkeys = [b['name'].lower().encode() for b in bundles]
    bmap, border = perfect_hash(bkeys)
    ckeys = [guid_bytes(c['guid']) for c in chunks]
    cmap, corder = perfect_hash(ckeys)
    names = bytearray()
    name_offsets = []
    for src in border:
        name_offsets.append(len(names))
        names += bundles[src]['name'].encode() + b'\x00'
    words = []
    ctable = bytearray()
    for src in corder:
        c = chunks[src]
        ctable += bytes(reversed(guid_bytes(c['guid'])))
        ctable += struct.pack('>I', (0x80 << 24) | len(words))
        ident = identifier(c['patch'], c['install'], c['archive'])
        words += [ident >> 32, ident & 0xFFFFFFFF, c['offset'], c['size']]
    pos = 48
    bundle_hash_off = pos
    pos = align(pos + len(bundles) * 4, 8)
    bundle_data_off = pos
    pos = align(pos + len(bundles) * 16, 4)
    chunk_hash_off = pos
    pos = align(pos + len(chunks) * 4, 4)
    chunk_guid_off = pos
    pos = align(pos + len(ctable), 4)
    chunk_data_off = pos
    pos += len(words) * 4
    names_off = pos
    pos = align(pos + len(names), 4)
    region_offs = []
    for src in border:
        region_offs.append(pos)
        pos = align(pos + len(bundles[src]['region']), 4)
    body = bytearray(pos)
    struct.pack_into('>IIiIIiIIIIiI', body, 0, bundle_hash_off, bundle_data_off, len(bundles),
                     chunk_hash_off if chunks else 0, chunk_guid_off if chunks else 0, len(chunks),
                     chunk_data_off, chunk_data_off, names_off, chunk_data_off, len(words), flags)
    for i, v in enumerate(bmap):
        struct.pack_into('>i', body, bundle_hash_off + 4 * i, v)
    for i, src in enumerate(border):
        b = bundles[src]
        struct.pack_into('>IIQ', body, bundle_data_off + 16 * i, name_offsets[i],
                         (b.get('load', 1) << 30) | len(b['region']), region_offs[i])
    if chunks:
        for i, v in enumerate(cmap):
            struct.pack_into('>i', body, chunk_hash_off + 4 * i, v)
        body[chunk_guid_off:chunk_guid_off + len(ctable)] = ctable
        for i, w in enumerate(words):
            struct.pack_into('>I', body, chunk_data_off + 4 * i, w)
    body[names_off:names_off + len(names)] = names
    for i, src in enumerate(border):
        r = bundles[src]['region']
        body[region_offs[i]:region_offs[i] + len(r)] = r
    envelope = bytearray(ENVELOPE)
    envelope[:4] = b'\x00\xd1\xce\x01'
    return bytes(envelope) + bytes(body)
