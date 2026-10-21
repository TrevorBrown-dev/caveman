"""Verifies a trick test mod (build_tricktest.py): verify_newdeck.py's checks with the trick states registered, then
the authored states themselves: each kernel decodes to the program exprkernel encodes for its trigger, each item
holds its value, their imports resolve to byte copies of the game's enum state and enum type carried in the mod, and
every state's file and instance GUIDs are equal (as the game's are).
Usage: verify_tricktest.py <mod folder> <game root>"""
import os
import runpy
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import ebx  # noqa: E402
import exprkernel  # noqa: E402
import fb  # noqa: E402
import newdeck  # noqa: E402
import tricktest  # noqa: E402
from gamedata import PKG, Game  # noqa: E402

mod, game_root = sys.argv[1], sys.argv[2]
tricktest.register()
base = runpy.run_path(os.path.join(HERE, 'verify_newdeck.py'), run_name='__main__')
fails, checks = list(base['fails']), base['checks']


def check(ok, what):
    global checks
    checks += 1
    if not ok:
        fails.append(what)
        print('  FAIL', what)


game = Game(game_root)
cas = open(os.path.join(mod, 'Win32', *PKG.split('/'), 'cas_01.cas'), 'rb').read()
toc = fb.read_toc(os.path.join(mod, 'Win32', 'items.toc'))
shared = next(b for b in toc['bundles'] if b['name'] == newdeck.SHARED)
_, files = fb.read_bundle_region(shared['region'])
m = fb.read_binary_bundle(cas[files[0]['offset']:files[0]['offset'] + files[0]['size']])
carried = {}
for i, e in enumerate(m['ebx']):
    f = files[1 + i]
    if f['patch'] and e['name'].startswith('animation/'):
        carried[e['name']] = fb.decode_cas(cas[f['offset']:f['offset'] + f['size']], game_root)[0]
print(f'\nstates carried in the shared bundle: {sorted(carried)}')
states = tricktest.TrickStates(game)
by_guid = {}
for name, data in carried.items():
    doc = ebx.read_document(data)
    by_guid[doc.file_guid] = (name, doc)
    check(doc.file_guid == doc.instances[0].instance_guid, f'{name}: file guid == instance guid')
    if name not in tricktest.AUTHORED:
        check(data == states.ebx(name)[0], f'{name}: a byte copy of the game\'s')
for name, (key, kind, state, enum_type, values) in tricktest.AUTHORED.items():
    doc = by_guid.get(next((d.file_guid for n, d in by_guid.values() if n == name), None))
    if doc is None:
        check(False, f'{name}: carried')
        continue
    doc = doc[1]
    check(doc.get('Name') == tricktest.display_name(key, kind), f'{name}: Name')
    target = by_guid.get(doc.imports[0][0])
    check(target is not None and target[0] == state and doc.imports[0] == (target[1].file_guid, target[1].instances[0].instance_guid),
          f'{name}: imports the carried {state}')
    if target is not None:
        et = by_guid.get(target[1].imports[0][0])
        check(et is not None and et[0] == enum_type, f'{state}: its enum type {enum_type} carried')
    if kind == 'kernel':
        words = exprkernel.words_of(doc)
        check(words == exprkernel.encode(values), f'{name}: kernel == encode({values})')
        check(doc.get('ExpressionKernelSizeBytes') == 4 * len(words), f'{name}: ExpressionKernelSizeBytes')
        d = exprkernel.decode(words)
        check(d['pool'][:len(values)] == values and len(d['instructions']) == 2 * len(values), f'{name}: {len(values)} value(s) decoded')
        print(f'  {name}: {exprkernel.describe(words).splitlines()[0]}')
        for line in exprkernel.describe(words).splitlines()[1:]:
            print(f'    {line}')
    else:
        check(doc.get('ItemValue') == values, f'{name}: ItemValue {values}')
        print(f'  {name}: ItemValue {doc.get("ItemValue")}')
print(f'\n{checks} checks, {len(fails)} failed (verify_newdeck plus the trick states)')
