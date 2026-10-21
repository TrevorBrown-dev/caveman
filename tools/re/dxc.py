"""DXC through the game's own dxcompiler.dll and dxil.dll (ctypes COM): disassemble, compile and sign DXIL, read containers.
Usage: dxc.py disasm <dxbc or shaders/bytecode file> [--game DIR]
       dxc.py compile <file.hlsl> <entry> <profile, e.g. ps_6_0> <out.dxbc> [--game DIR] [-- extra dxc args]"""
import ctypes
import os
import struct
import sys
import uuid
from ctypes import wintypes

GAME = r'E:\Games\ReSkate'
HRESULT = ctypes.c_long

CLSID_DxcCompiler = uuid.UUID('73e22d93-e6ce-47f3-b5bf-f0664f39c1b0')
CLSID_DxcOptimizer = uuid.UUID('ae2cd79f-cc22-453f-9b6b-b124e7a5204c')
IID_IDxcOptimizer = uuid.UUID('25740e2e-9cba-401b-9119-4fb42f39f270')
CLSID_DxcAssembler = uuid.UUID('d728db68-f903-4f80-94cd-dccf76ec7151')
IID_IDxcAssembler = uuid.UUID('091f7a26-1c1f-4948-904b-e6e3a8a771d5')
CLSID_DxcValidator = uuid.UUID('8ca3e215-f728-4cf3-8cdd-88af917587a1')
IID_IDxcValidator = uuid.UUID('a6e82bd2-1fd7-4826-9811-2857e797f49a')
VALIDATE_IN_PLACE = 1
CLSID_DxcUtils = uuid.UUID('6245d6af-66e0-48fd-80b4-4d271796748c')
IID_IDxcCompiler3 = uuid.UUID('228b4687-5a6a-4730-900c-9702b2203f54')
IID_IDxcUtils = uuid.UUID('4605c4cb-2019-492a-ada4-65f20bb7d67f')
IID_IDxcResult = uuid.UUID('58346cda-dde7-4497-9461-6f87af5e0659')
IID_IDxcBlob = uuid.UUID('8ba5fb08-5195-40e2-ac58-0d989c3a0102')
IID_IDxcBlobUtf8 = uuid.UUID('3da636c9-ba71-4024-a301-30cbf125305b')
DXC_OUT_OBJECT, DXC_OUT_ERRORS = 1, 2
SHADER_KINDS = {0: 'pixel', 1: 'vertex', 2: 'geometry', 3: 'hull', 4: 'domain', 5: 'compute', 13: 'mesh', 14: 'amplification'}


class GUID(ctypes.Structure):
    _fields_ = [('raw', ctypes.c_ubyte * 16)]

    @classmethod
    def of(cls, u):
        g = cls()
        ctypes.memmove(g.raw, u.bytes_le, 16)
        return g


class DxcBuffer(ctypes.Structure):
    _fields_ = [('Ptr', ctypes.c_void_p), ('Size', ctypes.c_size_t), ('Encoding', ctypes.c_uint)]


def _method(obj, index, restype, *argtypes):
    vtable = ctypes.cast(ctypes.cast(obj, ctypes.POINTER(ctypes.c_void_p))[0], ctypes.POINTER(ctypes.c_void_p))
    return ctypes.WINFUNCTYPE(restype, ctypes.c_void_p, *argtypes)(vtable[index])


def _check(hr, what):
    if hr < 0:
        raise OSError(f'{what} failed: HRESULT {hr & 0xffffffff:#010x}')


def _release(obj):
    if obj:
        _method(obj, 2, ctypes.c_ulong)(obj)


def _blob_bytes(blob):
    ptr = _method(blob, 3, ctypes.c_void_p)(blob)
    size = _method(blob, 4, ctypes.c_size_t)(blob)
    return ctypes.string_at(ptr, size) if size else b''


class Dxc:
    """The game's DXC. dxil.dll is loaded first from the game folder so compiles are validated and signed."""

    def __init__(self, game=GAME):
        self.dxil = ctypes.WinDLL(os.path.join(game, 'dxil.dll'))
        self.lib = ctypes.WinDLL(os.path.join(game, 'dxcompiler.dll'))
        create = self.lib.DxcCreateInstance
        create.restype = HRESULT
        create.argtypes = [ctypes.POINTER(GUID), ctypes.POINTER(GUID), ctypes.POINTER(ctypes.c_void_p)]
        self.compiler = ctypes.c_void_p()
        _check(create(GUID.of(CLSID_DxcCompiler), GUID.of(IID_IDxcCompiler3), ctypes.byref(self.compiler)), 'DxcCreateInstance(compiler)')
        self.utils = ctypes.c_void_p()
        _check(create(GUID.of(CLSID_DxcUtils), GUID.of(IID_IDxcUtils), ctypes.byref(self.utils)), 'DxcCreateInstance(utils)')
        self.optimizer = ctypes.c_void_p()
        _check(create(GUID.of(CLSID_DxcOptimizer), GUID.of(IID_IDxcOptimizer), ctypes.byref(self.optimizer)), 'DxcCreateInstance(optimizer)')
        self.assembler = ctypes.c_void_p()
        _check(create(GUID.of(CLSID_DxcAssembler), GUID.of(IID_IDxcAssembler), ctypes.byref(self.assembler)), 'DxcCreateInstance(assembler)')
        vcreate = self.dxil.DxcCreateInstance
        vcreate.restype = HRESULT
        vcreate.argtypes = [ctypes.POINTER(GUID), ctypes.POINTER(GUID), ctypes.POINTER(ctypes.c_void_p)]
        self.validator = ctypes.c_void_p()
        _check(vcreate(GUID.of(CLSID_DxcValidator), GUID.of(IID_IDxcValidator), ctypes.byref(self.validator)), 'dxil DxcCreateInstance(validator)')

    def _blob(self, data):
        """An IDxcBlobEncoding holding a copy of data (IDxcUtils::CreateBlob)."""
        blob = ctypes.c_void_p()
        _check(_method(self.utils, 6, HRESULT, ctypes.c_char_p, ctypes.c_uint32, ctypes.c_uint32, ctypes.POINTER(ctypes.c_void_p))(
            self.utils, data, len(data), 0, ctypes.byref(blob)), 'CreateBlob')
        return blob

    def _operation(self, result):
        """(output bytes, error text, status) of an IDxcOperationResult."""
        status = HRESULT()
        _check(_method(result, 3, HRESULT, ctypes.POINTER(HRESULT))(result, ctypes.byref(status)), 'GetStatus')
        out = b''
        blob = ctypes.c_void_p()
        if _method(result, 4, HRESULT, ctypes.POINTER(ctypes.c_void_p))(result, ctypes.byref(blob)) >= 0 and blob:
            out = _blob_bytes(blob)
            _release(blob)
        errors = b''
        err = ctypes.c_void_p()
        if _method(result, 5, HRESULT, ctypes.POINTER(ctypes.c_void_p))(result, ctypes.byref(err)) >= 0 and err:
            errors = _blob_bytes(err)
            _release(err)
        return out, errors.decode('utf-8', 'replace').rstrip('\0'), status.value

    def assemble(self, ir_text, sign=True):
        """DXIL LLVM assembly text (as dxc -disasm prints it) to a DXBC container: parsed by IDxcOptimizer (no passes),
        packed by IDxcAssembler, then validated and signed in place by dxil.dll. Returns (dxbc, messages, ok)."""
        src = self._blob(ir_text.encode() if isinstance(ir_text, str) else ir_text)
        module = ctypes.c_void_p()
        text = ctypes.c_void_p()
        hr = _method(self.optimizer, 5, HRESULT, ctypes.c_void_p, ctypes.POINTER(ctypes.c_wchar_p), ctypes.c_uint32,
                     ctypes.POINTER(ctypes.c_void_p), ctypes.POINTER(ctypes.c_void_p))(
            self.optimizer, src, None, 0, ctypes.byref(module), ctypes.byref(text))
        _release(src)
        log = _blob_bytes(text).decode('utf-8', 'replace').strip('\0').strip() if text else ''
        _release(text)
        if hr < 0 or not module:
            return b'', f'IR parse failed: HRESULT {hr & 0xffffffff:#010x} {log}', False
        result = ctypes.c_void_p()
        _check(_method(self.assembler, 3, HRESULT, ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p))(
            self.assembler, module, ctypes.byref(result)), 'AssembleToContainer')
        _release(module)
        dxbc, errors, status = self._operation(result)
        _release(result)
        if status < 0 or not dxbc:
            return b'', errors, False
        if sign:
            return self.validate(dxbc, sign=True)
        return dxbc, errors, True

    def validate(self, dxbc, sign=False):
        """Validates a container with dxil.dll; with sign=True writes the validator's hash into it.
        Returns (container, messages, ok)."""
        blob = self._blob(dxbc)
        result = ctypes.c_void_p()
        _check(_method(self.validator, 3, HRESULT, ctypes.c_void_p, ctypes.c_uint32, ctypes.POINTER(ctypes.c_void_p))(
            self.validator, blob, VALIDATE_IN_PLACE if sign else 0, ctypes.byref(result)), 'Validate')
        _, errors, status = self._operation(result)
        _release(result)
        signed = _blob_bytes(blob)
        _release(blob)
        return signed, errors, status >= 0

    def _result(self, result):
        """(object bytes, error text) of an IDxcResult."""
        status = HRESULT()
        _check(_method(result, 3, HRESULT, ctypes.POINTER(HRESULT))(result, ctypes.byref(status)), 'GetStatus')
        errors = b''
        err = ctypes.c_void_p()
        name = ctypes.c_void_p()
        if _method(result, 7, HRESULT, ctypes.c_uint, ctypes.POINTER(GUID), ctypes.POINTER(ctypes.c_void_p),
                   ctypes.POINTER(ctypes.c_void_p))(result, DXC_OUT_ERRORS, GUID.of(IID_IDxcBlobUtf8),
                                                   ctypes.byref(err), ctypes.byref(name)) >= 0 and err:
            errors = _blob_bytes(err)
            _release(err)
        _release(name)
        obj = b''
        if status.value >= 0:
            out = ctypes.c_void_p()
            # IDxcResult::GetResult (IDxcOperationResult slot 4)
            _check(_method(result, 4, HRESULT, ctypes.POINTER(ctypes.c_void_p))(result, ctypes.byref(out)), 'GetResult')
            if out:
                obj = _blob_bytes(out)
                _release(out)
        return obj, errors.decode('utf-8', 'replace').rstrip('\0'), status.value

    def disassemble(self, dxbc):
        buf = ctypes.create_string_buffer(dxbc, len(dxbc))
        db = DxcBuffer(ctypes.cast(buf, ctypes.c_void_p), len(dxbc), 0)
        result = ctypes.c_void_p()
        _check(_method(self.compiler, 4, HRESULT, ctypes.POINTER(DxcBuffer), ctypes.POINTER(GUID),
                       ctypes.POINTER(ctypes.c_void_p))(self.compiler, ctypes.byref(db), GUID.of(IID_IDxcResult),
                                                        ctypes.byref(result)), 'Disassemble')
        text, errors, status = self._result(result)
        _release(result)
        if status < 0:
            raise OSError(f'disassembly failed: {errors}')
        return text.decode('utf-8', 'replace').rstrip('\0')

    def compile(self, source, args):
        """Compiles HLSL text with dxc command-line style args (e.g. ['-E', 'main', '-T', 'ps_6_0']).
        Returns (dxbc bytes or b'', messages, ok). Validation and signing use the game's dxil.dll."""
        src = source.encode() if isinstance(source, str) else source
        buf = ctypes.create_string_buffer(src, len(src))
        db = DxcBuffer(ctypes.cast(buf, ctypes.c_void_p), len(src), 65001)
        argv = (ctypes.c_wchar_p * len(args))(*args)
        result = ctypes.c_void_p()
        _check(_method(self.compiler, 3, HRESULT, ctypes.POINTER(DxcBuffer), ctypes.POINTER(ctypes.c_wchar_p),
                       ctypes.c_uint32, ctypes.c_void_p, ctypes.POINTER(GUID), ctypes.POINTER(ctypes.c_void_p))(
            self.compiler, ctypes.byref(db), argv, len(args), None, GUID.of(IID_IDxcResult), ctypes.byref(result)), 'Compile')
        obj, errors, status = self._result(result)
        _release(result)
        return obj, errors, status >= 0


def container_parts(dxbc):
    """[(fourcc, offset of the part data, length)] of a DXBC container."""
    assert dxbc[:4] == b'DXBC', 'not a DXBC container'
    size, count = struct.unpack_from('<II', dxbc, 24)
    out = []
    for off in struct.unpack_from(f'<{count}I', dxbc, 32):
        out.append((dxbc[off:off + 4].decode('latin1'), off + 8, struct.unpack_from('<I', dxbc, off + 4)[0]))
    return out


def shader_kind(dxbc):
    """('pixel', 6, 0) etc. from the DXIL part's program version word."""
    part = next(p for p in container_parts(dxbc) if p[0] in ('DXIL', 'ILDB'))
    ver = struct.unpack_from('<I', dxbc, part[1])[0]
    return SHADER_KINDS.get(ver >> 16, ver >> 16), (ver >> 4) & 0xf, ver & 0xf


def read_bytecode_resource(data):
    """A `shaders/bytecode/<guid>` resource (type 0x0e2b02f7): header words, the RC2S block before the DXBC, and the DXBC."""
    at = data.find(b'DXBC')
    if at < 0:
        return {'dxbc': None}
    size = struct.unpack_from('<I', data, at + 24)[0]
    return {'header': data[:0x20], 'blob_size': struct.unpack_from('<I', data, 12)[0],
            'data_size': struct.unpack_from('<I', data, 16)[0], 'pre': data[0x20:at], 'dxbc_at': at,
            'dxbc': data[at:at + size], 'tail': data[at + size:]}


def main():
    args = sys.argv[1:]
    game = GAME
    if '--game' in args:
        i = args.index('--game'); game = args[i + 1]; del args[i:i + 2]
    extra = []
    if '--' in args:
        i = args.index('--'); extra = args[i + 1:]; args = args[:i]
    dxc = Dxc(game)
    if args[0] == 'disasm':
        data = open(args[1], 'rb').read()
        dxbc = data if data[:4] == b'DXBC' else read_bytecode_resource(data)['dxbc']
        print(dxc.disassemble(dxbc))
    elif args[0] == 'compile':
        src, entry, profile, out = args[1:5]
        obj, errors, ok = dxc.compile(open(src, encoding='utf-8').read(), ['-E', entry, '-T', profile] + extra)
        if errors:
            print(errors, file=sys.stderr)
        if not ok:
            raise SystemExit(1)
        open(out, 'wb').write(obj)
        print(f'{out}: {len(obj)} bytes, parts {[p[0] for p in container_parts(obj)]}')
    else:
        raise SystemExit(__doc__)


if __name__ == '__main__':
    main()
