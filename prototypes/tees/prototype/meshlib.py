"""Minimal MeshSet reader (port of ReSkate Engine/Resource/mesh_set.cpp) plus texcoords."""
import struct, uuid
import numpy as np
import fb

MESHSET = 0x49b156d4

def ptr(res, o):
    return struct.unpack_from('<Q', res, o)[0] + 16

def read_mesh(game, bundle, lod=0, res_name=None):
    m = bundle['manifest']
    ri = next(i for i, r in enumerate(m['res']) if r['type'] == MESHSET and (res_name is None or r['name'] == res_name))
    res = game.decode(bundle['files'][1 + len(m['ebx']) + ri])
    at = ptr(res, 48 + lod * 8)
    nsec = struct.unpack_from('<I', res, at + 8)[0]
    ifmt, ib, vb = struct.unpack_from('<III', res, at + 84)
    chunk = str(uuid.UUID(bytes_le=res[at + 116:at + 132]))
    c = game.toc()['chunk_by_guid'].get(chunk)
    if c is None:
        # chunk only in the bundle
        ci = next(i for i, cc in enumerate(m['chunks']) if cc['guid'] == chunk)
        geo = game.decode(bundle['files'][1 + len(m['ebx']) + len(m['res']) + ci])
    else:
        geo = game.decode(c)
    table = ptr(res, at + 12)
    out = {'pos': [], 'uv0': [], 'uv1': [], 'tri': [], 'sections': []}
    isz = 2 if ifmt == 33 else 4
    for si in range(nsec):
        s = table + si * 384
        tri, fi, vs, vc = struct.unpack_from('<IIII', res, s + 32)
        d = s + 112
        ne, ns = res[d + 96], res[d + 97]
        strides = [res[d + 64 + k * 2] for k in range(ns)]
        bases = []
        end = vs
        for st in strides:
            bases.append(end)
            end += st * vc
        attrs = {}
        for e in range(ne):
            usage, fmt, off, stream = res[d + e * 4:d + e * 4 + 4]
            attrs[usage] = (fmt, bases[stream], strides[stream], off)
        first = sum(len(p) for p in out['pos'])
        def col(usage, n, dtype):
            fmt, base, stride, off = attrs[usage]
            a = np.frombuffer(geo, dtype=np.uint8, count=stride * vc, offset=base).reshape(vc, stride)
            return np.ascontiguousarray(a[:, off:off + n * np.dtype(dtype).itemsize]).view(dtype).reshape(vc, n)
        out['pos'].append(col(1, 3, np.float32))
        for u, k in ((33, 'uv0'), (34, 'uv1')):
            if u in attrs:
                fmt = attrs[u][0]
                if fmt == 2:
                    out[k].append(col(u, 2, np.float16) if False else col(u, 2, np.float32))
        idx = np.frombuffer(geo, dtype=np.uint16 if isz == 2 else np.uint32, count=tri * 3, offset=vb + fi * isz).astype(np.int64)
        out['tri'].append(idx.reshape(-1, 3) + first)
        out['sections'].append((si, tri, vc, {u: a[0] for u, a in attrs.items()}))
    for k in ('pos', 'uv0', 'uv1', 'tri'):
        out[k] = np.concatenate(out[k]) if out[k] else None
    return out
