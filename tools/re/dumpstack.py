"""The faulting thread of a Skate.exe minidump: its registers, the module-relative return addresses on its stack,
and which threads hold caveman_fx frames. Usage: python -I dumpstack.py FILE.dmp"""
import logging
import os
import struct
import sys

logging.disable(logging.CRITICAL)
from minidump.minidumpfile import MinidumpFile  # noqa: E402

mf = MinidumpFile.parse(sys.argv[1])


def iv(x):
    try:
        return int(x)
    except Exception:
        return int(getattr(x, "value", 0))


mods = [(iv(m.baseaddress), iv(m.size), os.path.basename(m.name)) for m in mf.modules.modules]


def sym(a):
    for b, s, n in mods:
        if b <= a < b + s:
            return "%s+%#x" % (n, a - b)
    return None


raw_file = open(sys.argv[1], "rb")
segments = []
for seg in mf.memory_segments.memory_segments:
    segments.append((iv(seg.start_virtual_address), iv(seg.size), iv(seg.start_file_address)))
if getattr(mf, "memory_segments_64", None):
    for seg in mf.memory_segments_64.memory_segments:
        segments.append((iv(seg.start_virtual_address), iv(seg.size), iv(seg.start_file_address)))
print("memory segments", len(segments))


def read(a, n):
    for start, size, at in segments:
        if start <= a < start + size:
            n = min(n, start + size - a)
            raw_file.seek(at + (a - start))
            return raw_file.read(n)
    return b""


SKIP = ("ntdll", "KERNEL", "kernel", "ucrt", "msvcp", "vcruntime", "win32u", "USER32", "user32")
ex = mf.exception.exception_records[0]
# the x64 CONTEXT at the exception stream's location descriptor
loc = ex.ThreadContext
raw = open(sys.argv[1], "rb")
raw.seek(iv(loc.Rva))
cb = raw.read(iv(loc.DataSize))
REGS = {"Rax": 0x78, "Rcx": 0x80, "Rdx": 0x88, "Rbx": 0x90, "Rsp": 0x98, "Rbp": 0xA0, "Rsi": 0xA8, "Rdi": 0xB0, "R8": 0xB8, "R9": 0xC0, "R10": 0xC8, "R11": 0xD0, "R12": 0xD8, "R13": 0xE0, "R14": 0xE8, "R15": 0xF0, "Rip": 0xF8}
regs = {n: struct.unpack_from("<Q", cb, o)[0] for n, o in REGS.items()} if len(cb) >= 0x100 else {}
rsp, rip = regs.get("Rsp", 0), regs.get("Rip", 0)
print("faulting thread", ex.ThreadId, "rip", sym(rip), "rsp %#x" % rsp)
for n, v in regs.items():
    print("  %s %#x %s" % (n, v, sym(v) or ""))
if rsp == 0:
    for t in mf.threads.threads:
        if t.ThreadId == ex.ThreadId:
            rsp = iv(t.Stack.StartOfMemoryRange)
data = read(rsp, 0x6000)
print("stack bytes read", len(data))
seen = 0
for i in range(0, len(data) - 7, 8):
    q = struct.unpack_from("<Q", data, i)[0]
    s = sym(q)
    if s and not s.startswith(SKIP):
        print("  rsp+%#06x %s" % (i, s))
        seen += 1
        if seen > 40:
            break
print("threads holding caveman_fx frames:")
for t in mf.threads.threads:
    try:
        sp = iv(t.Stack.StartOfMemoryRange)
        sz = iv(t.Stack.MemoryRange.DataSize)
    except Exception:
        continue
    d = read(sp, min(sz or 0x4000, 0x20000))
    hits = []
    for i in range(0, len(d) - 7, 8):
        q = struct.unpack_from("<Q", d, i)[0]
        s = sym(q)
        if s and (s.startswith("caveman") or s.startswith("Skate.exe+0x133") or s.startswith("Skate.exe+0x131")):
            hits.append(s)
    if hits:
        print("  thread", t.ThreadId, "stack %#x" % sp, hits[:10])
