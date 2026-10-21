import sys, struct, uuid, json
sys.path.insert(0, r'C:\Users\thetr\Projects\ReSkate\Caveman\tools\re')
import ebx
from gamedata import Game
g = Game(r'E:\Games\ReSkate')
bn = g.find_bundle(sys.argv[1])
b = g.bundle(bn)
m = b['manifest']
ri = next(i for i, r in enumerate(m['res']) if r['type'] == 0x73312045)
res = g.decode(b['files'][1 + len(m['ebx']) + ri])
names = {}
for f in ('survey_tshirtrelaxed.json', 'survey_tops.json'):
    for ap, ps in json.load(open(f)).items():
        for pn, (key, val) in ps.items():
            names[uuid.UUID(key).bytes_le] = pn
# guid -> asset names in this bundle (file guids) and instance guids
fg = {}
for i, e in enumerate(m['ebx']):
    d = g.decode(b['files'][1 + i])
    doc = ebx.read_document(d)
    fg[uuid.UUID(str(doc.file_guid)).bytes_le] = e['name']
    for inst in doc.instances:
        if inst.exported:
            fg[uuid.UUID(str(inst.instance_guid)).bytes_le] = e['name'] + '#inst'
lo = 48
n = struct.unpack_from('<I', res, lo)[0]
at = lo + 4
out = []
for k in range(n):
    key = res[at:at + 16]
    th = key[8:12].hex()
    cnt = struct.unpack_from('<H', key, 12)[0]
    at += 16
    tag = res[at]
    # sizes by type hash
    if th == '30fcc514':      # texture
        size = 1 + 32
        v = res[at + 1:at + 33]
        val = fg.get(bytes(v[16:32]), fg.get(bytes(v[:16]), v.hex()))
    elif th == '2d5cb62b':    # float
        size = 1 + 4 + 4 * cnt if False else None
    out.append((names.get(bytes(key), key.hex()), th, cnt, tag, at))
    # generic: guess size by scanning to the next plausible key: try known sizes
    found = None
    for size in (5, 9, 13, 17, 21, 33, 2, 3):
        nxt = at + size
        if nxt + 16 <= len(res) and (k == n - 1 or res[nxt + 12:nxt + 14] in (b'\x01\x00', b'\x02\x00', b'\x03\x00', b'\x04\x00')):
            # check next key's type hash is one we've seen
            if res[nxt + 8:nxt + 12].hex() in ('30fcc514', '2d5cb62b', '59ff5e88', '7e814a8e', '4434ed36', 'd5e9362a'):
                found = size
                break
    if found is None:
        print('lost at', k, at); break
    raw = res[at:at + found]
    nm = names.get(bytes(key), key.hex())
    if th == '30fcc514':
        val = fg.get(bytes(raw[1:17]), None) or fg.get(bytes(raw[17:33]), raw.hex())
    elif th == '2d5cb62b':
        val = struct.unpack_from('<f', raw, found - 4)[0]
    elif th == '59ff5e88':
        val = struct.unpack_from('<3f', raw, found - 12)
    elif th == '4434ed36':
        val = struct.unpack_from('<I', raw, found - 4)[0]
    else:
        val = raw.hex()
    print(f'{nm:45s} {th} n{cnt} size{found} {val}')
    at += found
