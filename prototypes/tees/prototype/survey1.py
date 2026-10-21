"""Survey every stock top preset's parameters: name -> value summary."""
import sys, struct, collections, json
sys.path.insert(0, r'C:\Users\thetr\Projects\ReSkate\Caveman\tools\re')
import ebx
from gamedata import Game
g = Game(r'E:\Games\ReSkate')

def guid_names(bundle):
    out = {}
    m = bundle['manifest']
    for i, e in enumerate(m['ebx']):
        try:
            out[ebx.read_file_guid(g.decode(bundle['files'][1 + i]))] = e['name']
        except Exception:
            pass
    return out

def params(doc, names):
    records = {r.offset: r for r in doc.boxed_values}
    imports_at = set(doc.import_offsets)
    out = {}
    for v in doc.instances[0].object.find('ParamInfo').value.data:
        info = v.data
        key = str(info.find('ParamKey').value.data.find('Data').value.data)
        val = info.find('Value').value.data
        p = val.find('Parameter').value.data
        pname = names.get(doc.imports[p.index][0], '?').split('/')[-1] if p.kind == 'external' else '?'
        boxed = val.find('Value').value.data
        desc = None
        if isinstance(boxed, ebx.BoxedRef):
            rec = records.get(boxed.data_offset)
            if rec is None:
                desc = 'none'
            elif boxed.encoded_type & 0x80000000:
                t = ((boxed.encoded_type & 0x7FFFFFFF) >> 5) & 0x1F
                raw = rec.raw_bytes
                if t == ebx.FLOAT32: desc = round(struct.unpack_from('<f', raw)[0], 4)
                elif t == ebx.BOOL: desc = bool(raw[0])
                else: desc = f't{t}:{raw.hex()}'
            elif rec.offset in imports_at:
                idx = struct.unpack_from('<I', rec.raw_bytes)[0] >> 1
                fg, ig = doc.imports[idx]
                desc = '-> ' + names.get(fg, str(fg)) + (f' #{str(ig)[:8]}' if 'enum' in names.get(fg, '') else '')
            else:
                raw = rec.raw_bytes
                if len(raw) >= 12:
                    desc = 'vec ' + ','.join(f'{x:.3f}' for x in struct.unpack_from('<3f', raw))
                else:
                    desc = raw.hex()
        out[pname] = (key, desc)
    return out

if __name__ == '__main__':
    filt = sys.argv[1]
    names_all = [b for b in g.toc()['bundle_by_name'] if filt in b and b.endswith('_ap_cas_main_bundlereftable')]
    res = {}
    for bn in sorted(names_all):
        b = g.bundle(bn)
        nm = guid_names(b)
        ap = next(e['name'] for e in b['manifest']['ebx'] if e['name'].endswith('_ap') and e['name'] in bn)
        i = next(i for i, e in enumerate(b['manifest']['ebx']) if e['name'] == ap)
        doc = ebx.read_document(g.decode(b['files'][1 + i]))
        res[ap] = params(doc, nm)
    json.dump(res, open(sys.argv[2], 'w'), indent=1, default=str)
    print(len(res))
