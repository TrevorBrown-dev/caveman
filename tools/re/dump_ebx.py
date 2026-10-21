"""Prints an EBX asset's tree: root type, every instance with its type and fields, nested values, and
imports resolved to asset names where the bundle (or, with --deep, the whole TOC) has them.
Usage: dump_ebx.py <layer root: the game's Data or a mod folder> <toc relative to Win32> <bundle substring>
       <asset name> [--deep] [--max-array N]"""
import glob
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ebx  # noqa: E402
import fb  # noqa: E402
from gamedata import Game  # noqa: E402

args = [a for a in sys.argv[1:] if not a.startswith('--')]
root, toc_rel, bundle_part, asset = args[:4]
deep = '--deep' in sys.argv
max_array = int(sys.argv[sys.argv.index('--max-array') + 1]) if '--max-array' in sys.argv else 12
root = os.path.normpath(root)
is_base = os.path.basename(root).lower() == 'data'
game_root = os.path.dirname(root) if is_base else os.path.dirname(os.path.dirname(root))
game = Game(game_root)
layer = None if is_base else fb.Layer(root, game_root)


def read_file(f):
    raw = layer.read_raw(f) if (f['patch'] and layer) else game.raw(f)
    return fb.decode_cas(raw, game_root)[0]


def bundles(toc):
    for b in toc['bundles']:
        if b['region'] is None:
            continue
        inline, files = fb.read_bundle_region(b['region'])
        first = files[0]
        raw = layer.read_raw(first) if (first['patch'] and layer) else game.raw(first)
        try:
            m = fb.read_binary_bundle(raw)
        except Exception:
            m = fb.read_binary_bundle(fb.decode_cas(raw, game_root)[0])
        yield b['name'], m, files[1:]


toc_path = os.path.join(root, 'Win32', toc_rel)
toc = fb.read_toc(toc_path)
target = None
guid_names = {}
scan = []
for name, m, files in bundles(toc):
    if bundle_part in name:
        for i, e in enumerate(m['ebx']):
            if e['name'] == asset:
                target = (name, read_file(files[i]))
        scan.append((name, m, files))
    elif deep or not is_base:
        scan.append((name, m, files))
if target is None:
    sys.exit(f'{asset} not found in bundles matching {bundle_part!r}')
for name, m, files in scan:
    for i, e in enumerate(m['ebx']):
        try:
            guid_names[ebx.read_file_guid(read_file(files[i]))] = e['name']
        except Exception:
            pass

doc = ebx.read_document(target[1])
print(f'{asset}  (bundle {target[0]})')
print(f'root type {doc.root_type}, file guid {doc.file_guid}, {len(doc.instances)} instances, '
      f'{"EBXS" if doc.serialized_layout else "EBX"}, {"RFL2" if doc.hashed_reflection_names else "REFL"}')
print('imports:')
for k, (fg, ig) in enumerate(doc.imports):
    print(f'  [{k}] {guid_names.get(fg, "?")}  file {fg} instance {ig}')


imports_at = set(doc.import_offsets)
records = {r.offset: r for r in doc.boxed_values}
PRIMITIVE = {ebx.BOOL: '<?', ebx.INT32: '<i', ebx.UINT32: '<I', ebx.FLOAT32: '<f', ebx.ENUM: '<i', ebx.INT64: '<q',
             ebx.UINT64: '<Q', ebx.FLOAT64: '<d', ebx.INT8: '<b', ebx.UINT8: '<B', ebx.INT16: '<h', ebx.UINT16: '<H'}


def boxed(ref):
    """A boxed value: its type and what its bytes hold (an import, a primitive, or raw bytes)."""
    import struct
    rec = records.get(ref.data_offset)
    if rec is None:
        return f'boxed {ref.encoded_type:#x} (no record)'
    raw = rec.raw_bytes
    if ref.encoded_type & 0x80000000:
        t = ((ref.encoded_type & 0x7FFFFFFF) >> 5) & 0x1F
        fmt = PRIMITIVE.get(t)
        value = struct.unpack_from(fmt, raw)[0] if fmt and len(raw) >= struct.calcsize(fmt) else raw.hex()
        return f'boxed {ebx.TYPE_NAMES.get(t, t)} = {value}'
    if rec.offset in imports_at:
        index = struct.unpack_from('<I', raw)[0] >> 1
        fg, ig = doc.imports[index]
        return f'boxed reference -> import [{index}] {guid_names.get(fg, fg)} / {ig}'
    return f'boxed type {ref.encoded_type:#x} raw {raw.hex()}'


def show(v, indent):
    d = v.data
    pad = '  ' * indent
    if isinstance(d, ebx.Object):
        print(f' {doc.type_name(d.descriptor)}')
        for f in d.fields:
            print(f'{pad}  {f.name}:', end='')
            show(f.value, indent + 1)
    elif isinstance(d, list):
        print(f' [{len(d)}]')
        for k, e in enumerate(d[:max_array]):
            print(f'{pad}  [{k}]', end='')
            show(e, indent + 1)
        if len(d) > max_array:
            print(f'{pad}  ... {len(d) - max_array} more')
    elif isinstance(d, ebx.Pointer):
        if d.kind == 'internal':
            print(f' -> #{d.index} {doc.type_name(doc.instances[d.index].descriptor)}')
        elif d.kind == 'external':
            fg, ig = doc.imports[d.index]
            print(f' -> import [{d.index}] {guid_names.get(fg, fg)} / {ig}')
        else:
            print(' null')
    elif isinstance(d, ebx.BoxedRef):
        print(f' {boxed(d)}')
    elif isinstance(d, str):
        print(f' {d!r}')
    elif isinstance(d, float):
        print(f' {d:g}')
    else:
        print(f' {d!r}' if not isinstance(d, ebx.Guid) else f' {d}')


for k, inst in enumerate(doc.instances):
    t = doc.type_name(inst.descriptor)
    print(f'\n#{k} {t}' + (f'  guid {inst.instance_guid}' if inst.exported else '  (internal)'))
    for f in inst.object.fields:
        print(f'  {f.name}:', end='')
        show(f.value, 1)
