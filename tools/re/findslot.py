"""Where a code address appears as an 8-byte word in Skate.exe's image (vtable slots): python -I findslot.py RVA..."""
import struct
import sys

EXE = r"E:\Games\ReSkate\Skate.exe"
BASE = 0x140000000
data = open(EXE, "rb").read()
pe = struct.unpack_from("<I", data, 0x3C)[0]
nsec = struct.unpack_from("<H", data, pe + 6)[0]
opt = struct.unpack_from("<H", data, pe + 20)[0]
sections = []
for i in range(nsec):
    at = pe + 24 + opt + i * 40
    name = data[at:at + 8].rstrip(b"\0").decode("ascii", "replace")
    vsize, va, rsize, raw = struct.unpack_from("<IIII", data, at + 8)
    sections.append((name, va, vsize, raw, rsize))


def rva_of(off):
    for name, va, vsize, raw, rsize in sections:
        if raw <= off < raw + rsize:
            return name, va + (off - raw)
    return None, None


for arg in sys.argv[1:]:
    rva = int(arg, 16)
    needle = struct.pack("<Q", BASE + rva)
    print("exe+%#x:" % rva)
    at = 0
    while True:
        at = data.find(needle, at)
        if at < 0:
            break
        if at % 8 == 0:
            name, r = rva_of(at)
            print("  word at exe+%#x (%s, file %#x)" % (r, name, at))
        at += 1
