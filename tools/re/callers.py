"""Call sites (E8 rel32), jumps (E9 rel32) and RIP-relative lea/mov references to a code RVA in Skate.exe's .text:
python -I callers.py RVA"""
import struct
import sys

EXE = r"E:\Games\ReSkate\Skate.exe"
data = open(EXE, "rb").read()
pe = struct.unpack_from("<I", data, 0x3C)[0]
nsec = struct.unpack_from("<H", data, pe + 6)[0]
opt = struct.unpack_from("<H", data, pe + 20)[0]
text = None
for i in range(nsec):
    at = pe + 24 + opt + i * 40
    name = data[at:at + 8].rstrip(b"\0").decode("ascii", "replace")
    vsize, va, rsize, raw = struct.unpack_from("<IIII", data, at + 8)
    if name == ".text":
        text = (va, raw, rsize)
va, raw, rsize = text
target = int(sys.argv[1], 16)
code = data[raw:raw + rsize]
hits = []
for i in range(len(code) - 5):
    op = code[i]
    if op in (0xE8, 0xE9):
        rel = struct.unpack_from("<i", code, i + 1)[0]
        if va + i + 5 + rel == target:
            hits.append(("call" if op == 0xE8 else "jmp", va + i))
    elif op == 0x8D and i >= 1 and code[i - 1] in (0x48, 0x4C) and code[i + 1] in (0x05, 0x0D, 0x15, 0x1D, 0x25, 0x2D, 0x35, 0x3D):
        rel = struct.unpack_from("<i", code, i + 2)[0]
        if va + i + 6 + rel == target:
            hits.append(("lea", va + i - 1))
for kind, r in hits:
    print("%s at exe+%#x" % (kind, r))
print(len(hits), "reference(s)")
