"""Disassembles Skate.exe around RVAs. Usage: disasm.py <exe> <rva hex> [before] [after] ..."""
import struct
import sys

import capstone

exe = open(sys.argv[1], 'rb').read()
pe = struct.unpack_from('<I', exe, 0x3C)[0]
nsec = struct.unpack_from('<H', exe, pe + 6)[0]
opt = struct.unpack_from('<H', exe, pe + 20)[0]
base = struct.unpack_from('<Q', exe, pe + 24 + 24)[0]
sections = []
for i in range(nsec):
    o = pe + 24 + opt + 40 * i
    name = exe[o:o + 8].rstrip(b'\0').decode(errors='replace')
    vsize, va, rsize, rptr = struct.unpack_from('<IIII', exe, o + 8)
    sections.append((name, va, vsize, rptr, rsize))


def file_off(rva):
    for name, va, vsize, rptr, rsize in sections:
        if va <= rva < va + max(vsize, rsize):
            return rptr + rva - va, name
    return None, None


md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
args = sys.argv[2:]
for spec in args:
    rva = int(spec, 16)
    off, sec = file_off(rva)
    if off is None:
        print(f'{rva:#x}: not in any section'); continue
    start = off - 0x60
    code = exe[start: off + 0x40]
    print(f'== {rva:#x} in {sec} (file {off:#x}), image base {base:#x}')
    # Disassemble from a few starting points so the target instruction is decoded in sync.
    for back in range(0x60, 0, -1):
        insns = list(md.disasm(exe[off - back: off + 0x40], base + rva - back))
        if any(i.address == base + rva for i in insns):
            for i in insns:
                mark = '  <== ' if i.address == base + rva else '      '
                print(f'{mark}{i.address - base:#010x}: {i.mnemonic:8s} {i.op_str}')
            break
