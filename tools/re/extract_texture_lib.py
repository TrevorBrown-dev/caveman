import struct
# Frostbite ordinal -> (DXGI format, bytes per 4x4 block)
DXGI = {54: (71, 8), 55: (72, 8), 66: (98, 16), 67: (99, 16), 60: (77, 16), 61: (78, 16)}


def dds(width, height, dxgi, data):
    flags = 0x1 | 0x2 | 0x4 | 0x1000 | 0x80000
    header = struct.pack('<4sIIIIIII44x', b'DDS ', 124, flags, height, width, len(data), 0, 1)
    pf = struct.pack('<II4s20x', 32, 0x4, b'DX10')
    caps = struct.pack('<I16x', 0x1000)
    dx10 = struct.pack('<IIIII', dxgi, 3, 0, 1, 0)
    return header + pf + caps + dx10 + data


