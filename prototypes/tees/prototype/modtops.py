"""For each mod: the new clothing items, their category, asset paths, presets, and what each preset changes."""
import sys, os, struct, collections, json
sys.path.insert(0, r'C:\Users\thetr\Projects\ReSkate\Caveman\tools\re')
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ebx, fb
from gamedata import Game
from survey1 import params
G = r'E:\Games\ReSkate'
g = Game(G)
SH = 'win32/characters/customization/configs/cas_main_sharedbundle'
base_sh = g.bundle(SH)
base_names = {e['name'] for e in base_sh['manifest']['ebx']}
gnames = {}
def add_names(read, m, files):
    for i, e in enumerate(m['ebx']):
        try:
            gnames[ebx.read_file_guid(read(files[i]))] = e['name']
        except Exception:
            pass

def analyse(mod):
    layer = fb.Layer(mod, G)
    toc = fb.read_toc(os.path.join(mod, 'Win32', 'items.toc'))
    tocc = {c['guid']: c for c in toc['chunks']}
    def read(f):
        raw = layer.read_raw(f) if f['patch'] else g.raw(f)
        return fb.decode_cas(raw, G)[0]
    bundles = {}
    for b in toc['bundles']:
        m, files = layer.bundle(b['region'])
        bundles[b['name']] = (m, files)
        # names of all ebx (game + mod) in this bundle
        add_names(read, m, files)
    sm, sf = bundles[SH]
    out = []
    for i, e in enumerate(sm['ebx']):
        n = e['name']
        if n.startswith('items/') and n not in base_names and sf[i]['patch']:
            doc = ebx.read_document(read(sf[i]))
            if doc.root_type != 'DelMarBaseItemAsset':
                continue
            cats = [gnames.get(doc.imports[v.data.index][0], '?') for v in doc.get('Categories')] if doc.get('Categories') else []
            di = next((k for k, r in enumerate(doc.instances) if doc.type_name(r.descriptor) == 'DelMarCustomizationItemData'), None)
            paths = [] if di is None else [(doc.get(f'AssetPaths[{k}].AssetTypeId', di), doc.get(f'AssetPaths[{k}].AssetName', di)) for k in range(len(doc.get('AssetPaths', di)))]
            out.append({'item': n, 'key': doc.get('Key'), 'cats': cats, 'paths': paths})
    # presets
    for it in out:
        it['presets'] = []
        for tid, leaf in it['paths']:
            if tid != 3:
                continue
            for bn, (m, files) in bundles.items():
                for k, e in enumerate(m['ebx']):
                    if e['name'].endswith('/' + leaf.lower()) and files[k]['patch']:
                        doc = ebx.read_document(read(files[k]))
                        ps = params(doc, gnames)
                        texs = {}
                        for pn, (key, val) in ps.items():
                            if isinstance(val, str) and val.startswith('-> '):
                                tn = val[3:].split(' #')[0]
                                ri = next((j for j, r in enumerate(m['res']) if r['name'] == tn), None)
                                if ri is not None and files[len(m['ebx']) + ri]['patch']:
                                    h = fb.texture_header(read(files[len(m['ebx']) + ri]), m['res'][ri]['meta'])
                                    ci = next((j for j, c in enumerate(m['chunks']) if c['guid'] == h['chunk']), None)
                                    lo = m['chunks'][ci]['logical_offset'] if ci is not None else None
                                    texs[pn] = (tn.split('/')[-1], h['format'], h['w'], h['h'], h['mips'], h['first_mip'], hex(lo) if lo is not None else 'toc-only')
                        it['presets'].append({'bundle': bn.split('/')[-1], 'ap': e['name'], 'params': ps, 'new_textures': texs})
    return out

if __name__ == '__main__':
    res = {}
    for mod in sys.argv[2:]:
        try:
            res[os.path.basename(mod)] = analyse(mod)
        except Exception as ex:
            res[os.path.basename(mod)] = f'ERR {ex!r}'
        print(os.path.basename(mod), len(res[os.path.basename(mod)]) if isinstance(res[os.path.basename(mod)], list) else res[os.path.basename(mod)])
    json.dump(res, open(sys.argv[1], 'w'), indent=1, default=str)
