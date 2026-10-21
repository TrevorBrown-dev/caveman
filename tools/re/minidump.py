"""Reads a Windows minidump: the exception, the faulting thread's registers, and a heuristic stack walk
(stack values that point into loaded modules). Usage: minidump.py <dump>"""
import struct
import sys

data = open(sys.argv[1], 'rb').read()
sig, ver, nstreams, dir_rva = struct.unpack_from('<IIII', data, 0)
assert sig == 0x504D444D, 'not a minidump'
streams = {}
for i in range(nstreams):
    stype, size, rva = struct.unpack_from('<III', data, dir_rva + 12 * i)
    streams.setdefault(stype, []).append((size, rva))


def utf16_at(rva):
    n = struct.unpack_from('<I', data, rva)[0]
    return data[rva + 4: rva + 4 + n].decode('utf-16-le')


modules = []
for size, rva in streams.get(4, []):  # ModuleListStream
    n = struct.unpack_from('<I', data, rva)[0]
    for i in range(n):
        o = rva + 4 + 108 * i
        base, msize = struct.unpack_from('<QI', data, o)
        name_rva = struct.unpack_from('<I', data, o + 20)[0]
        modules.append((base, msize, utf16_at(name_rva).split('\\')[-1]))


def where(addr):
    for base, size, name in modules:
        if base <= addr < base + size:
            return f'{name}+{addr - base:#x}'
    return None


# memory ranges (MemoryListStream 5 and Memory64ListStream 9)
ranges = []
for size, rva in streams.get(5, []):
    n = struct.unpack_from('<I', data, rva)[0]
    for i in range(n):
        start, dsize, drva = struct.unpack_from('<QII', data, rva + 4 + 16 * i)
        ranges.append((start, dsize, drva))
for size, rva in streams.get(9, []):
    n, base_rva = struct.unpack_from('<QQ', data, rva)
    at = base_rva
    for i in range(n):
        start, dsize = struct.unpack_from('<QQ', data, rva + 16 + 16 * i)
        ranges.append((start, dsize, at))
        at += dsize


def read_mem(addr, n):
    for start, size, rva in ranges:
        if start <= addr and addr + n <= start + size:
            return data[rva + addr - start: rva + addr - start + n]
    return None


# exception
size, rva = streams[6][0]
tid = struct.unpack_from('<I', data, rva)[0]
code, flags, rec, eaddr, nparams = struct.unpack_from('<IIQQI', data, rva + 8)
params = struct.unpack_from('<15Q', data, rva + 8 + 32)[:nparams]
ctx_size, ctx_rva = struct.unpack_from('<II', data, rva + 8 + 152)
print(f'exception {code:#x} at {eaddr:#x} ({where(eaddr)}), thread {tid}, params {[hex(p) for p in params]}')
ctx = data[ctx_rva: ctx_rva + ctx_size]
regs = ['Rax', 'Rcx', 'Rdx', 'Rbx', 'Rsp', 'Rbp', 'Rsi', 'Rdi', 'R8', 'R9', 'R10', 'R11', 'R12', 'R13', 'R14', 'R15']
vals = struct.unpack_from('<16Q', ctx, 0x78)
rip = struct.unpack_from('<Q', ctx, 0xF8)[0]
print('rip', hex(rip), where(rip))
print('  '.join(f'{r}={v:#x}' + (f'({where(v)})' if where(v) else '') for r, v in zip(regs, vals)))
rsp = vals[4]
print('\nstack walk (values on the stack that point into modules):')
shown = 0
for off in range(0, 0x2000, 8):
    raw = read_mem(rsp + off, 8)
    if raw is None:
        break
    v = struct.unpack('<Q', raw)[0]
    w = where(v)
    if w and not w.startswith(('ntdll', 'KERNEL', 'KERNELBASE')) or (w and shown < 3):
        print(f'  rsp+{off:#06x}: {v:#x}  {w}')
        shown += 1
        if shown > 40:
            break
