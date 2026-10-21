"""Backward slice of a value in a DXIL disassembly: the instructions it is computed from, down to its texture samples,
constant-buffer loads and inputs. Usage: irslice.py <file.ll> <%value> [depth, default 12]"""
import re
import sys

DEF = re.compile(r'^\s*(%[\w.]+) = (.*)$')
LEAF = ('@dx.op.sample', '@dx.op.cbufferLoad', '@dx.op.loadInput', '@dx.op.textureLoad', '@dx.op.bufferLoad',
        '@dx.op.rawBufferLoad', '@dx.op.createHandle')


def definitions(ir):
    """{value: defining instruction text} of every SSA value."""
    out = {}
    for line in ir.splitlines():
        m = DEF.match(line)
        if m:
            out[m.group(1)] = m.group(2).split(';')[0].strip()
    return out


def operands(text):
    return re.findall(r'%[\w.]+', text)


def leaves(defs, value, seen=None):
    """The leaf instructions (samples, cbuffer loads, inputs) a value depends on, as {value: text}."""
    seen = set() if seen is None else seen
    out = {}
    stack = [value]
    while stack:
        v = stack.pop()
        if v in seen or v not in defs:
            continue
        seen.add(v)
        text = defs[v]
        if any(k in text for k in LEAF):
            out[v] = text
            if '@dx.op.sample' not in text and '@dx.op.textureLoad' not in text:
                continue
        stack.extend(o for o in operands(text) if o != v)
    return out


def tree(defs, value, depth, indent=0, seen=None):
    seen = set() if seen is None else seen
    if value not in defs:
        return [f'{"  " * indent}{value}']
    text = defs[value]
    lines = [f'{"  " * indent}{value} = {text[:150]}']
    if value in seen or depth == 0 or any(k in text for k in LEAF):
        return lines
    seen.add(value)
    for o in operands(text):
        if o != value and not o.startswith('%dx.types'):
            lines += tree(defs, o, depth - 1, indent + 1, seen)
    return lines


if __name__ == '__main__':
    ir = open(sys.argv[1]).read()
    defs = definitions(ir)
    depth = int(sys.argv[3]) if len(sys.argv) > 3 else 12
    print('\n'.join(tree(defs, sys.argv[2], depth)))
    print('\nleaves:')
    for v, t in leaves(defs, sys.argv[2]).items():
        print(f'  {v} = {t[:170]}')
