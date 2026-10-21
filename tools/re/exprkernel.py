"""Expression kernels: the bytecode inside the animation system's BooleanExpressionAsset (the game's `ebool.*`
states), decoded from the 215 kernels the game ships (knowledge/trick-triggers.md, 2026-10-21), and an encoder for
the one shape Caveman needs: "enum state == value" and "enum state in {values}", so a board effect can fire on one
specific trick (the current flip trick is an enum state, and effect triggers take a boolean).

The kernel is the asset's `ExpressionKernel` array of IntVec (4 x int32), `ExpressionKernelSizeBytes` = 16 x count:

  words  0..3   zero (runtime use)
  word   4      an id the compiler wrote (not a hash of the bytes with any common function; not in Skate.exe);
                equal across kernels with the same program, so it is a function of the program only
  words  5..7   -1
  word   8      64 (80 with three or four inputs): where the page block starts
  word   9      constant pool bytes (4 per constant, rounded up to 8)
  word  10      page 2 bytes: the temporaries (an enum or float temp is 4 bytes, a bool temp 1)
  word  11      word offset, from the start of the code, of the binding table (code words + 1)
  words 12..15  3 (pages), 0, 0, instruction count
  words 16..19  0, 0x00010200, 0, 0 (zeros in the one kernel with no inputs)
  then          the constant pool (page 0), the code, the binding table, and zero padding to
                align16(content) + 64 bytes

Code: instructions back to back, each `opword` (next instruction's byte offset << 8 | kind), 8 zero bytes (a slot
the runtime fills), then (page, byte offset) operand pairs, sources first and the destination last. Kind 0x19 is a
game-state load: (1, table) (1, input slot) -> dest, 36 bytes; kinds 1, 2, 3, 5, 6 are operators with that many
sources (28, 36, 44, 60, 68 bytes); kind 0x26 is a 16-byte conditional branch (cond pair, target offset). Pages:
0 the constant pool, 1 the inputs (an 8-byte slot per GameStateValueAsset at GameStateAssetOffsets, then the table
at GameStateTableOffset and the result at ResultRegisterOffset), 2 the temporaries. The binding table is the word
44 followed by (function hash, instruction byte offset) per instruction; the function hashes of the operators are
constants in Skate.exe, the loads' are not (those depend on the input's GameStateInterfaceTypes entry)."""
import copy
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ebx  # noqa: E402

# GameStateInterfaceTypes: the typed view a kernel asks a state asset for (the hashes are immediates in Skate.exe)
IFACE_BOOL, IFACE_FLOAT, IFACE_ENUM = 0xBCC2C469, 0x67745308, 0xA5EDBE98
# Function hashes from the binding tables, named from the kernels' use (the float compares by their constants)
OPS = {
    0x047FE1D5: 'load_bool', 0x70BAD5E7: 'load_enum', 0xE9298CAD: 'load_float', 0xAE764B0E: 'load_vec3',
    0x17B8026A: 'eq', 0x7072F0CC: 'ne', 0x1B74206C: 'not', 0x8EB2196D: 'and', 0xEABFEC6F: 'or',
    0xF5308775: 'or5', 0x93B3F2FB: 'and5', 0x02DD315A: 'float_gt?', 0xF3918EE3: 'float_lt?', 0xE6117A11: 'float_ge?',
    0x3042C4BD: 'float_le?', 0x41BE725E: 'abs?', 0x4575CF7D: 'in_range?', 0xCAADCD2E: 'xor?',
}
HASH = {v: k for k, v in OPS.items()}
LOAD, BRANCH = 0x19, 0x26
END_MARK = 44
TEMPLATE = 'animation/ebool.wipeout.force.ishard'   # the shared customization bundle (Dem Bones): "force == 2"
TEMPLATE_ID = 0x19E76286                             # its word 4 (shared with ebool.physics.inairstate: "state == 2")
TEMPLATE_ITEM = 'animation/dingo/enum.gsitem.filteredphysicsstate_air'   # root level: "filteredphysicsstate == 2"


def s32(x):
    return x - (1 << 32) if x & 0x80000000 else x


def align(x, a):
    return (x + a - 1) // a * a


def state_guid():
    """A GUID the way the game's state assets have them: one random 8-byte half, repeated (file = instance)."""
    h = os.urandom(8)
    return ebx.Guid(f'{h[0:4].hex()}-{h[4:6].hex()}-{h[6:8].hex()}-{h[0:2].hex()}-{h[2:8].hex()}')


# ---------------------------------------------------------------- decoding
def words_of(doc):
    """The kernel of a BooleanExpressionAsset document as a flat list of int32."""
    out = []
    for v in doc.instances[0].object.find('ExpressionKernel').value.data:
        out += [v.data.find(c).value.data for c in 'xyzw']
    return out


def decode(words):
    """{'id', 'pool' (int32 constants), 'temps' (bytes), 'instructions': [(name, kind, offset, next, operands)]}
    of a kernel. Operands are (page, offset) pairs; a branch's are (cond pair, target)."""
    pool_bytes, temps, table_word, n = words[9], words[10], words[11], words[15]
    code0 = 20 + pool_bytes // 4
    pool = words[20:code0]
    table = code0 + table_word
    assert words[table - 1] == END_MARK, 'no end mark before the binding table'
    bound = {words[table + 2 * i + 1]: words[table + 2 * i] & 0xFFFFFFFF for i in range(n)}
    out = []
    off, end = 0, (table_word - 1) * 4
    while off < end:
        w = code0 + off // 4
        opw = words[w] & 0xFFFFFFFF
        nxt, kind = opw >> 8, opw & 0xFF
        if kind == BRANCH:
            out.append(('branch', kind, off, nxt, [(words[w + 1], words[w + 2]), words[w + 3]]))
            off += 16
            continue
        # the length follows from the kind (a load has two sources, an operator `kind` of them, plus the
        # destination); `nxt` is where execution continues, a jump target after a branch
        length = 12 + 8 * (3 if kind == LOAD else kind + 1)
        ops = words[w + 3:w + length // 4]
        h = bound.get(off, 0)
        out.append((OPS.get(h, f'{h:08x}'), kind, off, nxt, [(ops[i], ops[i + 1]) for i in range(0, len(ops), 2)]))
        off += length
    return {'id': words[4] & 0xFFFFFFFF, 'pool': pool, 'temps': temps, 'instructions': out,
            'size': len(words) * 4, 'inputs_block': words[8]}


def describe(words):
    """A readable listing of a decoded kernel."""
    d = decode(words)
    lines = [f'id {d["id"]:08x}  pool {d["pool"]}  temps {d["temps"]} bytes  {d["size"]} bytes']
    for name, kind, off, nxt, ops in d['instructions']:
        lines.append(f'  {off:4d}: {name:10s} kind {kind:#04x}  {ops}  -> next {nxt}')
    return '\n'.join(lines)


# ---------------------------------------------------------------- encoding
def _instr(kind, nxt, pairs):
    return [(nxt << 8) | kind, 0, 0] + [x for p in pairs for x in p]


def encode(values, kernel_id=TEMPLATE_ID):
    """The kernel words of "enum input == values[0] or == values[1] or ...", laid out as the game's compiler lays out
    the same expressions (ebool.wipeout.force.ishard for one value, .issoft for two, ebool.traversaltype.is.vault for
    three): a load into a 4-byte temp, one `eq` per value, and a left-leaning chain of `or`. One input at page 1
    offset 0, the table at 8, the result at 16. `kernel_id` is word 4: the template's, since its derivation is
    unknown (it matched for every game kernel with the same program, so one constant per program shape at most)."""
    values = [int(v) for v in values]
    assert values, 'no values'
    pool_bytes = align(4 * len(values), 8)
    pool = values + [0] * (pool_bytes // 4 - len(values))
    code, binds = [], []
    off = 0
    result = (1, 16)

    def emit(kind, h, pairs):
        nonlocal off
        length = 12 + 8 * len(pairs)
        code.extend(_instr(kind, off + length, pairs))
        binds.append((h, off))
        off += length

    emit(LOAD, HASH['load_enum'], [(1, 8), (1, 0), (2, 0)])
    temp = 4
    if len(values) == 1:
        emit(2, HASH['eq'], [(2, 0), (0, 0), result])
    else:
        acc = (2, temp)
        emit(2, HASH['eq'], [(2, 0), (0, 0), acc])
        temp += 1
        for i in range(1, len(values)):
            t = (2, temp)
            emit(2, HASH['eq'], [(2, 0), (0, 4 * i), t])
            temp += 1
            last = i == len(values) - 1
            dest = result if last else (2, temp)
            emit(2, HASH['or'], [acc, t, dest])
            if not last:
                acc = dest
                temp += 1
    n = len(binds)
    header = [0, 0, 0, 0, s32(kernel_id), -1, -1, -1,
              64, pool_bytes, temp, len(code) + 1, 3, 0, 0, n,
              0, 0x00010200, 0, 0]
    table = [END_MARK] + [x for h, o in binds for x in (s32(h), o)]
    words = header + pool + code + table
    total = align(len(words) * 4, 16) + 64
    return words + [0] * (total // 4 - len(words))


def vectors(words):
    return [tuple(words[i:i + 4]) for i in range(0, len(words), 4)]


# ---------------------------------------------------------------- documents
def _clone_state(data, name):
    doc = ebx.read_document(data)
    g = state_guid()
    doc.set_file_guid(g)
    doc.set_instance_guid(0, g)
    doc.rename(name)
    return doc


def boolean_expression(template, name, state_ref, values, kernel_id=TEMPLATE_ID):
    """A new BooleanExpressionAsset "state in values" from TEMPLATE's bytes: `name` as the game writes it
    ('Animation/Caveman/EBool.Trick.Kickflip'; the manifest name is its lower case), `state_ref` the (file, instance)
    GUIDs of the enum state (GameStateEnumerationAsset). Returns the document."""
    doc = _clone_state(template, name)
    assert doc.root_type == 'BooleanExpressionAsset' and len(doc.imports) == 1, 'not a one-input expression template'
    doc.imports[0] = tuple(state_ref)
    o = doc.instances[0].object
    arr = o.find('ExpressionKernel').value
    proto = arr.data[0].data
    new = []
    for vec in vectors(encode(values, kernel_id)):
        e = copy.deepcopy(proto)
        for c, v in zip('xyzw', vec):
            e.find(c).value.data = v
        new.append(ebx.Value(e))
    arr.data[:] = new
    o.find('ExpressionKernelSizeBytes').value.data = 16 * len(new)
    assert [v.data for v in o.find('GameStateAssetOffsets').value.data] == [1, 0]
    assert [v.data & 0xFFFFFFFF for v in o.find('GameStateInterfaceTypes').value.data] == [IFACE_ENUM]
    return doc


def enum_item(template, name, state_ref, value):
    """A new GameStateEnumerationItemAsset "state == value" from TEMPLATE_ITEM's bytes: the game's own ready-made
    boolean over an enum state (used as a bool input by three of its kernels). Returns the document."""
    doc = _clone_state(template, name)
    assert doc.root_type == 'GameStateEnumerationItemAsset' and len(doc.imports) == 1
    doc.imports[0] = tuple(state_ref)
    doc.instances[0].object.find('ItemValue').value.data = int(value)
    return doc


# ---------------------------------------------------------------- self-test and CLI
def _game_kernel(game, name):
    import statefx
    sources = statefx.SOURCES + [('win32/characters/customization/configs/cas_main_sharedbundle', 'items.toc')]
    for b, t in sources:
        bundle = game.bundle(b, toc=t)
        m = bundle['manifest']
        i = next((i for i, e in enumerate(m['ebx']) if e['name'] == name), None)
        if i is not None:
            return game.decode(bundle['files'][1 + i])
    raise KeyError(name)


def selftest(game):
    """The encoder against the game's own kernels: byte-identical for one, two and three values."""
    cases = [('animation/ebool.wipeout.force.ishard', [2]), ('animation/ebool.wipeout.force.issoft', [1, 2]),
             ('animation/dingo/ebool.traversaltype.is.vault', [0, 6, 8])]
    for name, values in cases:
        doc = ebx.read_document(_game_kernel(game, name))
        theirs = words_of(doc)
        mine = encode(values, theirs[4] & 0xFFFFFFFF)
        same = mine == theirs
        print(f'{"ok " if same else "DIFF"} {name} {values}: {len(theirs)} words')
        if not same:
            for i in range(0, max(len(mine), len(theirs)), 4):
                a, b = mine[i:i + 4], theirs[i:i + 4]
                if a != b:
                    print(f'   [{i}] mine {a} theirs {b}')
    return True


if __name__ == '__main__':
    from gamedata import Game
    if len(sys.argv) < 2:
        sys.exit('usage: exprkernel.py <game root> [--selftest | <ebool asset name>]')
    g = Game(sys.argv[1])
    if len(sys.argv) == 2 or sys.argv[2] == '--selftest':
        selftest(g)
    else:
        print(describe(words_of(ebx.read_document(_game_kernel(g, sys.argv[2])))))
