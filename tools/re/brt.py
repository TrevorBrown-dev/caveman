"""Character bundle-reference tables (characters/customization/configs/cas_main_bundlereftable and its
kind, resource type 0x428ec9d4): which bundle to load for each preset, looked up by the FNV-1a hash of
the preset's full path and of its leaf name. Ported from ReSkate's Engine/Resource/bundle_ref_table.cpp
(GPL-3.0).

Layout (all little-endian), as the game ships it:
  header: 0 suffix string ptr, 8 lookups ptr, 16 path pool ptr (128), 24 bundle list ptr,
          32 empty string ptr, 72 lookup count, 76 bundle count, 92 version (1)
  pool:   path descriptors {u32 parent token, inline text or u32 offset}
  bundles: {u32 descriptor token, u32 flags} per bundle, index 0 a sentinel
  lookups: {u64 hash, u32 bundle index, u32 path token}, sorted by hash
  trailer: 5 relocations (the header pointers), 20 bytes
Resource meta is 16 bytes: payload size, trailer size (20), zero.

A cosmetic mod registers each preset it adds with a pair of rows (full path, leaf name) pointing at a
bundle the base already lists; the path text goes into the pool as an inline descriptor."""
import struct

TYPE = 0x428EC9D4
MAXIMUM_BYTES = 8 * 1024 * 1024
NONE = 0xFFFFFFFF


def fnv_hash(name):
    """64-bit FNV-1 (multiply, then xor) of the name, ASCII upper case folded to lower."""
    value = 0xCBF29CE484222325
    for c in name.encode('latin-1'):
        if 65 <= c <= 90:
            c += 32
        value = ((value * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF) ^ c
    return value


def leaf(path):
    return path.rsplit('/', 1)[-1]


def u32(b, at):
    if at < 0 or at + 4 > len(b):
        raise ValueError('truncated bundle-reference table')
    return struct.unpack_from('<I', b, at)[0]


def u64(b, at):
    if at < 0 or at + 8 > len(b):
        raise ValueError('truncated bundle-reference table')
    return struct.unpack_from('<Q', b, at)[0]


class Table:
    """A parsed table. `rows`: [(hash, bundle index, path token)] sorted by hash; `presets`: path ->
    bundle index; `bundle_names`: the bundle list's names, index 0 the sentinel."""

    def __init__(self, data, meta):
        data, meta = bytes(data), bytes(meta)
        if len(data) < 0x100 or len(data) > MAXIMUM_BYTES or len(meta) != 16:
            raise ValueError('not a bundle-reference table')
        self.data, self.meta = data, meta
        self.payload = len(data) - 20
        if u32(meta, 0) != self.payload or u32(meta, 4) != 20 or u64(meta, 8) != 0:
            raise ValueError('bundle-reference table metadata differs')
        for i in range(5):
            if u32(data, self.payload + i * 4) != i * 8:
                raise ValueError('bundle-reference relocations differ')
        self.pool = self._pointer(16)
        self.bundles = self._pointer(24)
        self.lookups = self._pointer(8)
        if (self.pool != 128 or self.bundles <= self.pool or self.lookups <= self.bundles or self.bundles & 15
                or self.lookups & 15 or u32(data, 92) != 1):
            raise ValueError('bundle-reference table layout differs')
        count = u32(data, 72)
        self.bundle_count = u32(data, 76)
        if (count > 200000 or not 2 <= self.bundle_count <= 100000
                or self.bundles + self.bundle_count * 8 != self.lookups or self.lookups + count * 16 != self.payload):
            raise ValueError('bundle-reference table counts differ')
        self._cache = {}
        self.rows = []
        self.presets = {}
        for i in range(count):
            at = self.lookups + i * 16
            row = (u64(data, at), u32(data, at + 8), u32(data, at + 12))
            if (self.rows and self.rows[-1][0] >= row[0]) or row[1] >= self.bundle_count:
                raise ValueError('bundle-reference lookups are not sorted or name no bundle')
            self.rows.append(row)
            path = self.text(row[2])
            if not path:
                raise ValueError('bundle-reference lookup has no path')
            self.presets.setdefault(path, row[1])
        self.bundle_entries = [(u32(data, self.bundles + 8 * i), u32(data, self.bundles + 8 * i + 4))
                               for i in range(self.bundle_count)]
        self.bundle_names = [self.text(tok) if tok != NONE and tok >> 24 else '' for tok, _ in self.bundle_entries]

    def _pointer(self, at):
        value = u64(self.data, at)
        if value >= self.payload:
            raise ValueError('bundle-reference pointer out of range')
        return value

    def text(self, token, depth=0):
        """The path a pool token names (its parent's text, then its own fragment)."""
        if token == NONE:
            return ''
        if depth > 128:
            raise ValueError('bundle-reference path is cyclic')
        if token in self._cache:
            return self._cache[token]
        at = self.pool + (token & 0x7FFFFF)
        length = token >> 24
        if not length or at > self.bundles or self.bundles - at < 8:
            raise ValueError('bad bundle-reference path')
        fragment = at + 4 if token & 0x800000 else self.pool + u32(self.data, at + 4)
        if fragment < self.pool or fragment > self.bundles or self.bundles - fragment < length:
            raise ValueError('bundle-reference path outside its pool')
        text = self.text(u32(self.data, at), depth + 1) + self.data[fragment:fragment + length].decode('ascii')
        if any(not 32 <= ord(c) <= 126 for c in text):
            raise ValueError('bundle-reference path is not text')
        self._cache[token] = text
        return text

    def bundle_index(self, name):
        """The index of a bundle in the list, by its name in the list (the stock preset path, such as
        characters/.../deck_gen_graphic_00010/2023/deck_gen_graphic_00010_ap) or by its items.toc name
        (win32/<that>_cas_main_bundlereftable); case-insensitive. None when not listed."""
        name = name.lower()
        if name.startswith('win32/') and name.endswith('_cas_main_bundlereftable'):
            name = name[len('win32/'):-len('_cas_main_bundlereftable')]
        for i, n in enumerate(self.bundle_names):
            if n.lower() == name:
                return i
        return None


def add_presets(table, presets):
    """A copy of `table` with each (preset path, bundle name) added, as a cosmetic mod ships it.
    Returns (resource bytes, meta). Raises when a path is already a lookup or a bundle is not listed."""
    builder = Builder(table)
    for path, bundle in presets:
        index = table.bundle_index(bundle)
        if index is None:
            raise ValueError(f'{bundle} is not in the bundle list')
        builder.insert(path, index)
    return builder.write()


class Builder:
    """A table being added to, as ReSkate's merge (and a cosmetic mod) adds presets: each new path is
    appended to the pool as an inline descriptor, with a full-path row and, unless another preset
    already answers to it, a leaf-name row, both pointing at an existing bundle."""

    def __init__(self, table):
        self.table = table
        self.rows = list(table.rows)
        self.hashes = {r[0] for r in self.rows}
        self.leaves = {}
        for path in sorted(table.presets):  # in path order: each leaf keeps the first preset
            self.leaves.setdefault(fnv_hash(leaf(path)), path)
        self.pool = bytearray()
        self.size = len(table.data)
        self.changed = False

    def insert(self, path, bundle):
        """Adds `path` in bundle index `bundle`. Returns the preset that keeps the leaf name when this
        one only got its full-path row, else ''."""
        if self.size > MAXIMUM_BYTES:
            raise ValueError('not a bundle-reference table')
        if len(self.rows) > 200000:
            raise ValueError('bundle-reference table counts differ')
        full, short = fnv_hash(path), fnv_hash(leaf(path))
        if full in self.hashes:
            raise ValueError(f'{path} collides with an existing lookup')
        with_leaf = full != short and short not in self.hashes
        holder = ''
        if full != short and not with_leaf:
            holder = self.leaves.get(short, 'another lookup')
        raw = path.encode('ascii')
        if len(raw) > 255:
            raise ValueError(f'{path} is too long for a lookup')
        growth = (4 + len(raw) + 15) & ~15
        offset = self.table.bundles + len(self.pool) - self.table.pool
        if offset > 0x7FFFFF:
            raise ValueError('bundle-reference path pool is full')
        token = offset | 0x800000 | (len(raw) << 24)
        at = len(self.pool)
        self.pool += bytes(growth)
        struct.pack_into('<I', self.pool, at, NONE)
        self.pool[at + 4:at + 4 + len(raw)] = raw
        self.rows.append((full, bundle, token))
        self.hashes.add(full)
        if with_leaf:
            self.rows.append((short, bundle, token))
            self.hashes.add(short)
        held = self.leaves.get(short)
        if held is None or path < held:
            self.leaves[short] = path
        self.size += growth + (32 if with_leaf else 16)
        self.changed = True
        return holder

    def write(self):
        """(resource bytes, resource meta): the base's own bytes when nothing was added."""
        t = self.table
        if not self.changed:
            return t.data, t.meta
        bundles = t.bundles + len(self.pool)
        lookups = t.lookups + len(self.pool)
        payload = lookups + len(self.rows) * 16
        out = bytearray(payload + 20)
        out[:t.bundles] = t.data[:t.bundles]
        out[t.bundles:bundles] = self.pool
        out[bundles:lookups] = t.data[t.bundles:t.lookups]
        for i, (h, b, tok) in enumerate(sorted(self.rows, key=lambda r: r[0])):
            struct.pack_into('<QII', out, lookups + i * 16, h, b, tok)
        out[payload:] = t.data[t.payload:]
        struct.pack_into('<Q', out, 8, lookups)
        struct.pack_into('<Q', out, 24, bundles)
        struct.pack_into('<I', out, 72, len(self.rows))
        meta = bytearray(t.meta)
        struct.pack_into('<I', meta, 0, payload)
        return bytes(out), bytes(meta)


def added_presets(base, copy):
    """Presets a copy adds over the base, in the order their descriptors sit in the copy's pool
    (the order they were inserted). Raises when the copy moves or drops a base preset or changes the
    bundle list, as ReSkate's merge would."""
    if copy.data[copy.bundles:copy.lookups] != base.data[base.bundles:base.lookups]:
        raise ValueError('the copy changes the bundle list')
    for path, b in base.presets.items():
        if copy.presets.get(path) != b:
            raise ValueError(f'the copy moves or drops {path}')
    tokens = {}
    for h, b, tok in copy.rows:
        path = copy.text(tok)
        if path not in base.presets:
            tokens.setdefault(path, (tok & 0x7FFFFF, b))
    return [(p, b) for p, (_, b) in sorted(tokens.items(), key=lambda kv: kv[1][0])]


def merge(base, copies):
    """ReSkate's merge: every preset any copy adds, the first copy winning a path. Returns
    (resource, meta, added, conflicts, shadowed) where shadowed is [(copy index, path, holder)]."""
    builder = Builder(base)
    added, conflicts, shadowed = {}, 0, []
    for index, copy in enumerate(copies):
        for path, b in sorted(added_presets(base, copy)):
            if path in added:
                conflicts += added[path] != b
                continue
            holder = builder.insert(path, b)
            if holder:
                shadowed.append((index, path, holder))
            added[path] = b
    data, meta = builder.write()
    return data, meta, len(added), conflicts, shadowed
