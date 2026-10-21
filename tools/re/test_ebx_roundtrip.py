"""Round-trips EBX through ebx.read_document / ebx.write_document and compares bytes.
Usage: test_ebx_roundtrip.py <game root> [base sample size, default 3000] [--mods-only | --base-only] [--edits]
--edits also rewrites each deck-related document compactly, renamed and with new GUIDs, and checks it
reads back with exactly that change.
Mod EBX: every mod-owned EBX in the cosmetic mods under <game root>/Mods (map mods are skipped).
Base EBX: a sample from Data/Win32/items.toc, deck-related bundles first."""
import glob
import os
import sys
import time
import traceback

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ebx  # noqa: E402
import fb  # noqa: E402
from gamedata import Game  # noqa: E402

game_root = sys.argv[1]
sample = int(sys.argv[2]) if len(sys.argv) > 2 and sys.argv[2].isdigit() else 3000
DECK_WORDS = ('board_bottomart', 'items/', 'thumbnail/', 'deck_', 'itemmastercollection')


def deck_related(name):
    n = name.lower()
    return any(w in n for w in DECK_WORDS)


def tree(doc):
    """A comparable rendering of every instance's values (layout-independent)."""
    def val(v):
        d = v.data
        if isinstance(d, ebx.Object):
            return ('obj', doc.type_name(d.descriptor), tuple((f.name, val(f.value)) for f in d.fields))
        if isinstance(d, list):
            return ('arr', tuple(val(e) for e in d))
        if isinstance(d, ebx.Pointer):
            return ('ptr', d.kind, d.index)
        if isinstance(d, ebx.ResourceRef):
            return ('res', d.id)
        if isinstance(d, ebx.TypeRef):
            return ('type', d.encoded)
        if isinstance(d, ebx.BoxedRef):
            return ('box', d.encoded_type)
        if isinstance(d, float) and d != d:
            return ('nan',)
        return d
    return (str(doc.file_guid), tuple(doc.import_list()),
            tuple((doc.type_name(i.descriptor), str(i.instance_guid), val(ebx.Value(i.object))) for i in doc.instances))


def edit_check(data):
    """Compact rewrite, rename and new GUIDs: each must read back with exactly the intended change."""
    doc = ebx.read_document(data)
    before = tree(doc)
    if tree(ebx.read_document(ebx.write_document(doc, preserve_layout=False))) != before:
        return 'compact rewrite changed values'
    if doc.instances[0].object.find('Name') is None:
        return None
    new_guid = ebx.Guid.new()
    doc.rename(doc.get('Name') + '_caveman_test_rename')
    doc.set_file_guid(new_guid)
    doc.set_instance_guid(0, new_guid)
    for layout in (True, False):
        again = ebx.read_document(ebx.write_document(doc, preserve_layout=layout))
        if again.get('Name') != doc.get('Name') or again.file_guid != new_guid or again.instances[0].instance_guid != new_guid:
            return f'edit did not read back (preserve_layout={layout})'
        a, b = tree(again), tree(doc)
        if a != b:
            return f'edit changed other values (preserve_layout={layout})'
    return None


class Tally:
    def __init__(self, label):
        self.label = label
        self.counts = {'identical': 0, 'different': 0, 'failed': 0}
        self.deck = {'identical': 0, 'different': 0, 'failed': 0}
        self.edits = {'checked': 0, 'failed': 0}
        self.examples = []

    def add(self, name, data):
        try:
            out = ebx.write_document(ebx.read_document(data))
            kind = 'identical' if out == data else 'different'
            detail = ''
            if kind == 'different':
                at = next((i for i in range(min(len(out), len(data))) if out[i] != data[i]), min(len(out), len(data)))
                detail = f'first difference at {at:#x} (sizes {len(data)} -> {len(out)})'
        except Exception as e:  # noqa: BLE001
            kind, detail = 'failed', f'{type(e).__name__}: {e}'
        self.counts[kind] += 1
        if deck_related(name):
            self.deck[kind] += 1
            if kind == 'identical' and '--edits' in sys.argv:
                problem = edit_check(data)
                self.edits['checked'] += 1
                if problem:
                    self.edits['failed'] += 1
                    if len(self.examples) < 12:
                        self.examples.append(('edit', name, problem))
        if kind != 'identical' and len(self.examples) < 12:
            self.examples.append((kind, name, detail))

    def report(self):
        total = sum(self.counts.values())
        dtotal = sum(self.deck.values())
        print(f'\n== {self.label}: {total} EBX | identical {self.counts["identical"]}, different {self.counts["different"]}, '
              f'failed {self.counts["failed"]}')
        print(f'   deck-related: {dtotal} | identical {self.deck["identical"]}, different {self.deck["different"]}, '
              f'failed {self.deck["failed"]}')
        if self.edits['checked']:
            print(f'   edit checks (compact rewrite, rename, new GUIDs): {self.edits["checked"]} | '
                  f'failed {self.edits["failed"]}')
        for kind, name, detail in self.examples:
            print(f'   {kind}: {name}: {detail}')


t0 = time.time()
if '--base-only' not in sys.argv:
    mods = Tally('mod-owned EBX (cosmetic mods)')
    for mod in sorted(glob.glob(os.path.join(game_root, 'Mods', '*', ''))):
        name = os.path.basename(os.path.dirname(mod))
        if name.startswith('.') or os.path.exists(os.path.join(mod, 'reskate-levels.json')):
            continue
        layer = fb.Layer(mod, game_root)
        for toc_path in glob.glob(os.path.join(mod, 'Win32', '**', '*.toc'), recursive=True):
            try:
                toc = fb.read_toc(toc_path)
            except Exception:
                continue
            for b in toc['bundles']:
                if b['region'] is None:
                    continue
                try:
                    m, files = layer.bundle(b['region'])
                except Exception:
                    continue
                for i, e in enumerate(m['ebx']):
                    f = files[i]
                    if not f['patch']:
                        continue
                    try:
                        data = fb.decode_cas(layer.read_raw(f), game_root)[0]
                    except Exception:
                        continue
                    mods.add(f'{name}: {e["name"]}', data)
    mods.report()
    print(f'   ({time.time() - t0:.0f}s)')

if '--mods-only' not in sys.argv:
    t1 = time.time()
    base = Tally(f'base-game EBX sample (items.toc)')
    game = Game(game_root)
    names = sorted(game.toc()['bundle_by_name'])
    deck_first = [n for n in names if '/deck/' in n or 'sharedbundle' in n] + [n for n in names if '/deck/' not in n]
    seen = 0
    for bname in deck_first:
        if seen >= sample:
            break
        try:
            b = game.bundle(bname)
        except Exception:
            continue
        m = b['manifest']
        for i, e in enumerate(m['ebx']):
            if seen >= sample:
                break
            if 'sharedbundle' in bname and not deck_related(e['name']):
                continue
            try:
                data = game.decode(b['files'][1 + i])
            except Exception:
                continue
            base.add(e['name'], data)
            seen += 1
    base.report()
    print(f'   ({time.time() - t1:.0f}s)')
