"""Lists every superbundle, bundle and asset a mod folder ships. Usage: dump_mod.py <mod dir> <game root> [--textures]"""
import glob
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import fb  # noqa: E402

mod, game = sys.argv[1], sys.argv[2]
show_tex = '--textures' in sys.argv
layer = fb.Layer(mod, game)
print('cas archives:', {k: [r for r, _ in v] for k, v in layer.cas.items()})
for toc_path in sorted(glob.glob(os.path.join(mod, 'Win32', '**', '*.toc'), recursive=True)):
    toc = fb.read_toc(toc_path)
    print(f'\n### {os.path.relpath(toc_path, mod)}: flags={toc["flags"]} bundles={len(toc["bundles"])} chunks={len(toc["chunks"])}')
    for c in toc['chunks'][:400]:
        print('  toc chunk', c)
    for b in toc['bundles']:
        print(f'\n  BUNDLE {b["name"]} (load={b["load"]}, region {b["size"]} bytes)')
        if b['region'] is None:
            continue
        m, files = layer.bundle(b['region'])
        print(f'    manifest endian={m["endian"]} ebx={len(m["ebx"])} res={len(m["res"])} chunks={len(m["chunks"])} chunk_meta={len(m["chunk_meta"])}B files={len(files)}')
        locs = {}
        for f in files:
            k = (f['patch'], f['install'], f['archive'])
            locs[k] = locs.get(k, 0) + 1
        print('    file locations (patch, install chunk, archive): count =', locs)
        assets = [('ebx', a) for a in m['ebx']] + [('res', a) for a in m['res']] + [('chunk', a) for a in m['chunks']]
        for i, (kind, a) in enumerate(assets):
            f = files[i] if i < len(files) else None
            where = f"{'P' if f['patch'] else 'B'}{f['install']}:{f['archive']}@{f['offset']}+{f['size']}" if f else '?'
            extra = ''
            if kind == 'res':
                extra = f" type={a['type']:#x} rid={a['rid']:#x} meta={a['meta'].hex()}"
            if kind == 'chunk':
                extra = f" lo={a['logical_offset']:#x} ls={a['logical_size']}"
            label = a.get('name') if kind != 'chunk' else a['guid']
            print(f'    {kind:5} {label} size={a.get("size", "")}{extra} [{where}]')
            if show_tex and kind == 'res' and a['type'] == fb.TEXTURE_TYPE and f and f['patch']:
                try:
                    raw = layer.read_raw(f)
                    payload, blocks = fb.decode_cas(raw, game)
                    print('          TEXTURE', fb.texture_header(payload, a['meta']))
                except Exception as e:
                    print('          texture read failed:', e)
