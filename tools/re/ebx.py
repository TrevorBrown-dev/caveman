"""EBX documents (RIFF EBX/EBXS with EBXD, EFIX, EBXX and REFL/RFL2 chunks): read, edit, write.
A port of ReSkate's Engine/Resource/ebx_document.cpp and ebx_writer.cpp (GPL-3.0).

read_document(bytes) -> Document; write_document(Document) -> bytes. Values hold Python types:
bool, int, float, str, Guid, Sha1, ResourceRef, Pointer, TypeRef, BoxedRef, Object and list
(an array, with its record hash on Value.array_hash)."""
import struct
import uuid

RIFF, EBX, EBXS = b'RIFF', b'EBX\x00', b'EBXS'

# Field types ((encoded >> 4) & 0x1F) and categories (encoded & 0x0F).
INHERITED, DBOBJECT, STRUCTURE, POINTER, ARRAY, FIXED_ARRAY, STRING, CSTRING, ENUM, FILEREF = range(10)
BOOL, INT8, UINT8, INT16, UINT16, INT32, UINT32, UINT64, INT64, FLOAT32, FLOAT64 = range(10, 21)
GUID, SHA1, RESOURCEREF, FUNCTION, TYPEREF, BOXEDVALUEREF, INTERFACE, DELEGATE = range(21, 29)
TYPE_NAMES = {v: k.lower() for k, v in dict(
    INHERITED=0, DBOBJECT=1, STRUCTURE=2, POINTER=3, ARRAY=4, FIXED_ARRAY=5, STRING=6, CSTRING=7, ENUM=8,
    FILEREF=9, BOOL=10, INT8=11, UINT8=12, INT16=13, UINT16=14, INT32=15, UINT32=16, UINT64=17, INT64=18,
    FLOAT32=19, FLOAT64=20, GUID=21, SHA1=22, RESOURCEREF=23, FUNCTION=24, TYPEREF=25, BOXEDVALUEREF=26,
    INTERFACE=27, DELEGATE=28).items()}
CAT_NONE, CAT_POINTER, CAT_STRUCTURE, CAT_PRIMITIVE, CAT_ARRAY = range(5)

SCALAR_SIZE = {BOOL: 1, INT8: 1, UINT8: 1, INT16: 2, UINT16: 2, INT32: 4, UINT32: 4, FLOAT32: 4, ENUM: 4,
               INT64: 8, UINT64: 8, FLOAT64: 8, RESOURCEREF: 8, POINTER: 8, CSTRING: 8, FILEREF: 8,
               TYPEREF: 8, DELEGATE: 8, GUID: 16, SHA1: 20, STRING: 32, BOXEDVALUEREF: 16}
INT_FORMATS = {INT8: '<b', UINT8: '<B', INT16: '<h', UINT16: '<H', INT32: '<i', ENUM: '<i', UINT32: '<I',
               INT64: '<q', UINT64: '<Q'}


def _scalar_alignment(t):
    # A GUID is four-byte aligned in the game's own arrays (ReSkate's writer assumes 8).
    if t == GUID:
        return 4
    return max(min(SCALAR_SIZE.get(t, 8), 8), 1)


def align_up(value, alignment):
    return value if alignment <= 1 else (value + alignment - 1) // alignment * alignment


def _text(raw):
    return raw.decode('utf-8', 'surrogateescape')


def _raw(text):
    return text.encode('utf-8', 'surrogateescape')


# ---------------------------------------------------------------- value types
class Guid(bytes):
    """16 bytes in the native (little-endian) GUID layout; str() gives the usual text form."""
    def __new__(cls, value=bytes(16)):
        if isinstance(value, str):
            value = uuid.UUID(value).bytes_le
        assert len(value) == 16
        return super().__new__(cls, value)

    def __str__(self):
        return str(uuid.UUID(bytes_le=bytes(self)))

    __repr__ = lambda self: f'Guid({str(self)!r})'

    @classmethod
    def new(cls):
        return cls(uuid.uuid4().bytes_le)


class Sha1(bytes):
    __repr__ = lambda self: f'Sha1({self.hex()})'


class F32Bits(float):
    """A float32 whose exact bits are kept (NaN payloads, infinities)."""
    def __new__(cls, bits):
        self = super().__new__(cls, struct.unpack('<f', struct.pack('<I', bits))[0])
        self.bits = bits
        return self


class F64Bits(float):
    def __new__(cls, bits):
        self = super().__new__(cls, struct.unpack('<d', struct.pack('<Q', bits))[0])
        self.bits = bits
        return self


class ResourceRef:
    """`listed`: whether the file's fixup table named this slot (the game leaves null references
    out; ReSkate's writer lists every one). None for a new reference, which is listed."""
    __slots__ = ('id', 'listed')

    def __init__(self, id=0, listed=None):
        self.id, self.listed = id, listed

    def __repr__(self):
        return f'ResourceRef({self.id:#x})'


class Pointer:
    """kind: 'null', 'internal' (index into Document.instances) or 'external' (index into imports)."""
    __slots__ = ('kind', 'index')

    def __init__(self, kind='null', index=-1):
        self.kind, self.index = kind, index

    def __repr__(self):
        return f'Pointer({self.kind}, {self.index})'


class TypeRef:
    __slots__ = ('primitive', 'primitive_type', 'descriptor', 'encoded')

    def __init__(self, primitive=False, primitive_type=0, descriptor=-1, encoded=0):
        self.primitive, self.primitive_type, self.descriptor, self.encoded = primitive, primitive_type, descriptor, encoded

    def __repr__(self):
        return f'TypeRef({self.encoded:#x})'


class BoxedRef:
    __slots__ = ('encoded_type', 'data_offset', 'relative_offset')

    def __init__(self, encoded_type=0, data_offset=-1, relative_offset=0):
        self.encoded_type, self.data_offset, self.relative_offset = encoded_type, data_offset, relative_offset

    def __repr__(self):
        return f'BoxedRef({self.encoded_type:#x}, {self.data_offset})'


class Value:
    """`array_hash`: an array's EBXX record hash. `source_offset`: where an array read from a file sat in
    its EBXD (the writer keeps arrays in that order, so an unchanged document writes back byte for byte)."""
    __slots__ = ('data', 'array_hash', 'source_offset')

    def __init__(self, data=None, array_hash=None, source_offset=None):
        self.data, self.array_hash, self.source_offset = data, array_hash, source_offset

    def __repr__(self):
        return f'Value({self.data!r})'


class FieldValue:
    __slots__ = ('descriptor', 'name', 'value')

    def __init__(self, descriptor, name, value):
        self.descriptor, self.name, self.value = descriptor, name, value


class Object:
    __slots__ = ('descriptor', 'fields')

    def __init__(self, descriptor=-1, fields=None):
        self.descriptor, self.fields = descriptor, fields if fields is not None else []

    def find(self, name):
        name = name.lower()
        return next((f for f in self.fields if f.name.lower() == name), None)


# ---------------------------------------------------------------- descriptors and records
class TypeDescriptor:
    __slots__ = ('name', 'name_hash', 'field_index', 'field_count', 'encoded_type', 'size', 'alignment',
                 'guid', 'signature')

    def type(self):
        return (self.encoded_type >> 4) & 0x1F

    def category(self):
        return self.encoded_type & 0x0F


class FieldDescriptor:
    __slots__ = ('name', 'name_hash', 'encoded_type', 'class_ref', 'data_offset')

    def type(self):
        return (self.encoded_type >> 4) & 0x1F

    def category(self):
        return self.encoded_type & 0x0F


class InstanceRecord:
    __slots__ = ('fixup_type', 'descriptor', 'data_offset', 'exported', 'instance_guid', 'object', 'raw_image')

    def __init__(self, fixup_type, descriptor, data_offset, exported):
        self.fixup_type, self.descriptor, self.data_offset, self.exported = fixup_type, descriptor, data_offset, exported
        self.instance_guid = Guid()
        self.object = None
        self.raw_image = b''


class ArrayRecord:
    __slots__ = ('offset', 'count', 'hash', 'encoded_type', 'class_ref')

    def __init__(self, offset, count, hash, encoded_type, class_ref):
        self.offset, self.count, self.hash, self.encoded_type, self.class_ref = offset, count, hash, encoded_type, class_ref


class BoxedValueRecord(ArrayRecord):
    __slots__ = ('raw_bytes',)

    def __init__(self, offset, count, hash, encoded_type, class_ref, raw_bytes=b''):
        super().__init__(offset, count, hash, encoded_type, class_ref)
        self.raw_bytes = raw_bytes


class Document:
    def __init__(self):
        self.file_guid = Guid()
        self.root_type = ''
        self.data_start = self.data_end = 0
        self.arrays_offset = self.boxed_values_offset = self.strings_offset = 0
        self.serialized_layout = False
        self.hashed_reflection_names = False
        self.reflection_data = b''
        self.fixup_type_guids = []
        self.fixup_type_signatures = []
        self.boxed_reference_offsets = []
        self.types = []
        self.fields = []
        self.instances = []
        self.imports = []            # [(file Guid, class/instance Guid)]
        self.arrays = []
        self.boxed_values = []
        self.pointer_offsets = []
        self.resource_ref_offsets = []
        self.import_offsets = []
        self.type_info_offsets = []
        self.boxed_value_slots = []
        self.payload = b''           # the EBXD bytes as read (offsets as in the fixup tables)
        self.reflection_groups = []  # [(hash, count, first)]

    def root(self):
        return self.instances[0] if self.instances else None

    # ---- helpers
    def type_name(self, descriptor):
        return self.types[descriptor].name if 0 <= descriptor < len(self.types) else '?'

    def instances_of(self, type_name):
        """[(index, InstanceRecord)] whose type is `type_name` (case-insensitive)."""
        t = type_name.lower()
        return [(i, r) for i, r in enumerate(self.instances) if self.type_name(r.descriptor).lower() == t]

    def get(self, path, instance=0):
        """A field value by path from an instance's object: 'Name', 'ItemData.Thumbnail', 'Values[2].Key'.
        Returns the Python value (Value.data)."""
        return _walk(self.instances[instance].object, path).data

    def set(self, path, data, instance=0):
        """Sets a field value by path. `data` must be the type the field holds (str, int, Guid, ...)."""
        _walk(self.instances[instance].object, path).data = data

    def strings(self):
        """[(instance index, path, text)] of every string value in the document."""
        out = []
        for i, r in enumerate(self.instances):
            if r.object is not None:
                for path, v in _values(r.object, ''):
                    if isinstance(v.data, str):
                        out.append((i, path, v.data))
        return out

    def import_list(self):
        """[(file guid text, instance guid text)] of the imports, in order."""
        return [(str(f), str(c)) for f, c in self.imports]

    def rename(self, name):
        """Sets the asset's own name (the root object's Name field)."""
        field = self.instances[0].object.find('Name')
        if field is None:
            raise KeyError('the root object has no Name field')
        field.value.data = name

    def set_file_guid(self, guid):
        self.file_guid = Guid(guid) if not isinstance(guid, Guid) else guid

    def set_instance_guid(self, index, guid):
        self.instances[index].instance_guid = Guid(guid) if not isinstance(guid, Guid) else guid

    def remap_imports(self, mapping):
        """Replaces import GUIDs by {old Guid (file or instance): new Guid}."""
        self.imports = [(mapping.get(f, f), mapping.get(c, c)) for f, c in self.imports]


def _walk(obj, path):
    value = None
    for part in path.replace(']', '').split('.'):
        name, *indices = part.split('[')
        field = obj.find(name)
        if field is None:
            raise KeyError(f'no field {name!r} in {path!r}')
        value = field.value
        for index in indices:
            value = value.data[int(index)]
        if isinstance(value.data, Object):
            obj = value.data
    return value


def _values(obj, prefix):
    for f in obj.fields:
        path = f'{prefix}{f.name}'
        yield from _value_paths(f.value, path)


def _value_paths(v, path):
    yield path, v
    if isinstance(v.data, Object):
        yield from _values(v.data, path + '.')
    elif isinstance(v.data, list):
        for i, e in enumerate(v.data):
            yield from _value_paths(e, f'{path}[{i}]')


# ---------------------------------------------------------------- reading
class _Reader:
    def __init__(self, data):
        self.b = data
        self.pos = 0

    def seek(self, pos):
        if pos > len(self.b):
            raise ValueError('reader seek is out of range')
        self.pos = pos

    def take(self, n):
        if self.pos + n > len(self.b):
            raise ValueError('reader ran past the end')
        out = self.b[self.pos:self.pos + n]
        self.pos += n
        return out

    def unpack(self, fmt):
        v = struct.unpack_from(fmt, self.b, self.pos)
        self.pos += struct.calcsize(fmt)
        return v[0] if len(v) == 1 else v

    def u8(self): return self.unpack('<B')
    def u16(self): return self.unpack('<H')
    def u32(self): return self.unpack('<I')
    def i32(self): return self.unpack('<i')
    def u64(self): return self.unpack('<Q')
    def guid(self): return Guid(self.take(16))

    def c_string(self):
        end = self.b.index(b'\x00', self.pos)
        out = self.b[self.pos:end]
        self.pos = end + 1
        return _text(out)


def _chunks(r):
    if r.take(4) != RIFF:
        raise ValueError('EBX is not a RIFF document')
    riff_end = r.u32() + 8
    if riff_end != len(r.b):
        raise ValueError('RIFF EBX size does not match its stream')
    form = r.take(4)
    if form not in (EBX, EBXS):
        raise ValueError('RIFF document is not EBX or EBXS')
    chunks = []
    while r.pos < riff_end:
        cid = r.take(4)
        size = r.u32()
        begin = r.pos
        if size > riff_end - begin:
            raise ValueError('RIFF chunk exceeds its document')
        chunks.append((cid, begin, begin + size))
        r.seek(begin + size + (size & 1))
    return chunks, form


def _chunk(chunks, cid, required=True):
    found = [c for c in chunks if c[0] == cid]
    if len(found) > 1:
        raise ValueError(f'duplicate RIFF {cid!r} chunk')
    if not found and required:
        raise ValueError(f'missing RIFF {cid!r} chunk')
    return found[0] if found else None


def _check(doc, offset, required, label):
    size = doc.data_end - doc.data_start
    if offset > size or required > size - offset:
        raise ValueError(f'{label} offset exceeds EBXD')


def _read_fixup(r, chunk, doc):
    r.seek(chunk[1])
    doc.file_guid = r.guid()
    n = r.i32()
    guids = [r.guid() for _ in range(n)]
    n = r.i32()
    if n > len(guids):
        raise ValueError('EFIX has more signatures than types')
    sigs = [r.u32() for _ in range(n)]
    doc.fixup_type_guids, doc.fixup_type_signatures = guids, sigs
    exported, count = r.i32(), r.i32()
    if exported > count:
        raise ValueError('EFIX exported instance count is invalid')
    for index in range(count):
        offset = r.u32()
        _check(doc, offset, 2, 'EFIX instance')
        class_ref = struct.unpack_from('<H', r.b, doc.data_start + offset)[0]
        if class_ref >= len(guids):
            raise ValueError('EFIX instance type is invalid')
        doc.instances.append(InstanceRecord(class_ref, -1, offset, index < exported))

    def offsets(label, width):
        out = []
        for _ in range(r.i32()):
            offset = r.u32()
            _check(doc, offset, width, label)
            out.append(offset)
        return out

    doc.pointer_offsets = offsets('EFIX pointer', 8)
    doc.resource_ref_offsets = offsets('EFIX resource reference', 8)
    doc.imports = [(r.guid(), r.guid()) for _ in range(r.i32())]
    doc.import_offsets = offsets('EFIX import relocation', 8)
    doc.type_info_offsets = offsets('EFIX type-info reference', 8)
    doc.arrays_offset, doc.boxed_values_offset, doc.strings_offset = r.u32(), r.u32(), r.u32()
    for o, label in ((doc.arrays_offset, 'array'), (doc.boxed_values_offset, 'boxed-value'), (doc.strings_offset, 'string')):
        _check(doc, o, 0, f'EFIX {label} section')
    doc._has_boxed_table = r.pos < chunk[2]
    if doc._has_boxed_table:
        doc.boxed_reference_offsets = offsets('EFIX boxed reference', 16)
    if r.pos != chunk[2]:
        raise ValueError('unexpected bytes at the end of EFIX')
    return guids, sigs


def _read_extra(r, chunk, doc):
    r.seek(chunk[1])
    na, nb = r.i32(), r.i32()
    for _ in range(na):
        offset, count, h, et, cr = r.unpack('<IiIHH')
        _check(doc, offset, 0, 'EBXX array')
        doc.arrays.append(ArrayRecord(offset, count, h, et, cr))
    for _ in range(nb):
        offset, count, h, et, cr = r.unpack('<IiIHH')
        _check(doc, offset, 0, 'EBXX boxed value')
        doc.boxed_values.append(BoxedValueRecord(offset, count, h, et, cr))
    if r.pos != chunk[2]:
        raise ValueError('unexpected bytes at the end of EBXX')


def _string_at(b, offset, end):
    stop = b.index(b'\x00', offset)
    if stop >= end:
        raise ValueError('reflection string is unterminated')
    return _text(b[offset:stop])


def _read_reflection(r, chunk, hashed, doc):
    r.seek(chunk[1])
    ids = [(r.guid(), r.u32()) for _ in range(r.i32())]
    type_names = []
    for index in range(r.i32()):
        t = TypeDescriptor()
        t.name_hash, t.field_index, t.field_count, et, t.size, t.alignment = r.unpack('<IiHHHH')
        t.encoded_type = et >> 1
        t.guid, t.signature = ids[index] if index < len(ids) else (Guid(), 0)
        type_names.append(t.name_hash)
        doc.types.append(t)
    field_names = []
    for _ in range(r.i32()):
        f = FieldDescriptor()
        f.name_hash, f.data_offset, et, f.class_ref = r.unpack('<IIHH')
        f.encoded_type = et >> 1
        field_names.append(f.name_hash)
        doc.fields.append(f)
    for t in doc.types:
        if t.field_index < 0 or t.field_index > len(doc.fields) or t.field_count > len(doc.fields) - t.field_index:
            raise ValueError('reflection type field range is invalid')
    doc.reflection_groups = [r.unpack('<III') for _ in range(r.i32())]
    mappings = r.i32()
    r.seek(r.pos + 8 * mappings)
    if hashed:
        name_offsets = {}
        for _ in range(r.i32()):
            h, o = r.unpack('<II')
            name_offsets[h] = o
        strings = r.pos
        for t, h in zip(doc.types, type_names):
            t.name = _string_at(r.b, strings + name_offsets[h], chunk[2])
        for f, h in zip(doc.fields, field_names):
            f.name = _string_at(r.b, strings + name_offsets[h], chunk[2])
    else:
        strings = r.pos
        for t, h in zip(doc.types, type_names):
            t.name = _string_at(r.b, strings + h, chunk[2])
        for f, h in zip(doc.fields, field_names):
            f.name = _string_at(r.b, strings + h, chunk[2])
    return ids


class _ValueReader:
    def __init__(self, b, doc):
        self.b, self.doc = b, doc
        self.r = _Reader(b)
        self.array_hashes = {a.offset: a.hash for a in doc.arrays}
        self.instance_at = {inst.data_offset: i for i, inst in enumerate(doc.instances)}
        self.resource_slots = set(doc.resource_ref_offsets)
        self.depth = 0

    def read_instances(self):
        doc = self.doc
        for inst in doc.instances:
            if not 0 <= inst.descriptor < len(doc.types):
                continue
            start = doc.data_start + inst.data_offset
            if inst.exported:
                inst.instance_guid = Guid(self.b[start - 16:start])
            size = doc.types[inst.descriptor].size
            if start + size > doc.data_end:
                raise ValueError('EBX object exceeds EBXD')
            inst.raw_image = self.b[start:start + size]
            inst.object = self.read_object(inst.descriptor, start)

    def read_object(self, descriptor, start):
        self.depth += 1
        if self.depth > 128:
            raise ValueError('EBX types nest too deeply')
        t = self.doc.types[descriptor]
        if start + t.size > self.doc.data_end:
            raise ValueError('EBX object exceeds EBXD')
        obj = Object(descriptor)
        self.read_fields(obj, t, start)
        self.depth -= 1
        return obj

    def read_fields(self, obj, t, start):
        doc = self.doc
        for k in range(t.field_count):
            di = t.field_index + k
            f = doc.fields[di]
            if f.type() == INHERITED:
                self.depth += 1
                self.read_fields(obj, doc.types[f.class_ref], start)
                self.depth -= 1
                continue
            pos = start + f.data_offset
            if pos > doc.data_end:
                raise ValueError('EBX field offset exceeds EBXD')
            value = self.read_field(f, pos)
            if f.category() == CAT_ARRAY:
                target = pos + struct.unpack_from('<i', self.b, pos)[0] - doc.data_start
                value.source_offset = target
                if value.data:
                    h = self.array_hashes.get(target)
                    if h is not None:
                        value.array_hash = h
            obj.fields.append(FieldValue(di, f.name, value))

    def read_field(self, f, pos):
        doc, r = self.doc, self.r
        if f.category() == CAT_ARRAY:
            disp = struct.unpack_from('<i', self.b, pos)[0]
            target = pos + disp
            if target < doc.data_start + 4 or target > doc.data_end:
                raise ValueError('EBX array displacement exceeds EBXD')
            count = struct.unpack_from('<i', self.b, target - 4)[0]
            if count < 0 or count > doc.data_end - target:
                raise ValueError('EBX array count is invalid')
            r.seek(target)
            values = []
            ft = f.type()
            for _ in range(count):
                element = r.pos
                values.append(self.read_value(ft, f.class_ref, element))
                if ft == STRUCTURE:
                    r.seek(element + doc.types[f.class_ref].size)
                elif ft in (POINTER, CSTRING):
                    r.seek(align_up(r.pos, 8))
            return Value(values)
        r.seek(pos)
        return self.read_value(f.type(), f.class_ref, pos)

    def read_value(self, t, class_ref, pos):
        r = self.r
        if t == BOOL:
            return Value(r.u8() != 0)
        if t in INT_FORMATS:
            return Value(r.unpack(INT_FORMATS[t]))
        if t == FLOAT32:
            bits = r.u32()
            v = struct.unpack('<f', struct.pack('<I', bits))[0]
            return Value(v if v == v and abs(v) != float('inf') else F32Bits(bits))
        if t == FLOAT64:
            bits = r.u64()
            v = struct.unpack('<d', struct.pack('<Q', bits))[0]
            return Value(v if v == v and abs(v) != float('inf') else F64Bits(bits))
        if t == GUID:
            return Value(r.guid())
        if t == SHA1:
            return Value(Sha1(r.take(20)))
        if t == RESOURCEREF:
            return Value(ResourceRef(r.u64(), pos - self.doc.data_start in self.resource_slots))
        if t == STRING:
            raw = r.take(32)
            return Value(_text(raw.split(b'\x00', 1)[0]))
        if t == CSTRING:
            disp = r.u32()
            return Value(self.relative_string(pos, disp), source_offset=self.string_source(pos, disp))
        if t == FILEREF:
            disp = r.u32()
            r.pos += 4
            return Value(self.relative_string(pos, disp), source_offset=self.string_source(pos, disp))
        if t == POINTER:
            encoded = r.i32()
            if encoded == 0:
                return Value(Pointer('null', -1))
            if encoded & 1:
                index = encoded >> 1
                if not 0 <= index < len(self.doc.imports):
                    raise ValueError('EBX pointer import is invalid')
                return Value(Pointer('external', index))
            target = pos + encoded - self.doc.data_start
            if target not in self.instance_at:
                raise ValueError('EBX internal pointer target is invalid')
            return Value(Pointer('internal', self.instance_at[target]))
        if t in (TYPEREF, DELEGATE):
            encoded = r.u32()
            r.pos += 4
            if encoded == 0:
                return Value(TypeRef())
            if encoded & 0x80000000:
                return Value(TypeRef(True, ((encoded & 0x7FFFFFFF) >> 5) & 0x1F, -1, encoded))
            return Value(TypeRef(False, 0, encoded >> 2, encoded))
        if t == BOXEDVALUEREF:
            encoded = r.u32()
            r.pos += 4
            disp = r.unpack('<q')
            target = pos + 8 + disp - self.doc.data_start
            self.doc.boxed_value_slots.append(pos - self.doc.data_start)
            return Value(BoxedRef(encoded, target, disp))
        if t == STRUCTURE:
            return Value(self.read_object(class_ref, pos))
        return Value()

    def string_source(self, base, disp):
        return None if disp == 0xFFFFFFFF else base + disp - self.doc.data_start

    def relative_string(self, base, disp):
        if disp == 0xFFFFFFFF:
            return ''
        target = base + disp
        if target < self.doc.data_start or target >= self.doc.data_end:
            raise ValueError('EBX string displacement exceeds EBXD')
        end = self.b.index(b'\x00', target)
        if end >= self.doc.data_end:
            raise ValueError('EBX string exceeds EBXD')
        return _text(self.b[target:end])


def read_document(data):
    data = bytes(data)
    r = _Reader(data)
    chunks, form = _chunks(r)
    ebxd = _chunk(chunks, b'EBXD')
    efix = _chunk(chunks, b'EFIX')
    ebxx = _chunk(chunks, b'EBXX')
    refl = _chunk(chunks, b'REFL', False)
    rfl2 = _chunk(chunks, b'RFL2', False)
    if (refl is None) == (rfl2 is None):
        raise ValueError('RIFF EBX must contain exactly one REFL or RFL2 chunk')
    doc = Document()
    doc.serialized_layout = form == EBXS
    doc.data_start = align_up(ebxd[1], 16)
    doc.data_end = ebxd[2]
    if doc.data_start > doc.data_end:
        raise ValueError('EBXD alignment exceeds the chunk')
    doc._chunk_order = [c[0] for c in chunks]
    doc._ebxd_pad = data[ebxd[1]:doc.data_start]
    fix_guids, fix_sigs = _read_fixup(r, efix, doc)
    _read_extra(r, ebxx, doc)
    rc = rfl2 or refl
    ids = _read_reflection(r, rc, rfl2 is not None, doc)
    doc.hashed_reflection_names = rfl2 is not None
    doc.reflection_data = data[rc[1]:rc[2]]
    doc.payload = data[doc.data_start:doc.data_end]
    id_index = {}
    for i, key in enumerate(ids):
        id_index.setdefault(key, i)
    for inst in doc.instances:
        if inst.fixup_type < len(fix_guids) and inst.fixup_type < len(fix_sigs):
            inst.descriptor = id_index.get((fix_guids[inst.fixup_type], fix_sigs[inst.fixup_type]), -1)
    if doc.instances and 0 <= doc.instances[0].descriptor < len(doc.types):
        doc.root_type = doc.types[doc.instances[0].descriptor].name
    _ValueReader(data, doc).read_instances()
    doc.boxed_values.sort(key=lambda v: v.offset)
    for i, v in enumerate(doc.boxed_values):
        end = doc.boxed_values[i + 1].offset if i + 1 < len(doc.boxed_values) else doc.strings_offset
        if end < v.offset:
            raise ValueError('EBXX boxed values are not ordered')
        v.raw_bytes = data[doc.data_start + v.offset:doc.data_start + end]
    return doc


def read_file_guid(data):
    r = _Reader(bytes(data))
    chunks, _ = _chunks(r)
    return Guid(r.b[_chunk(chunks, b'EFIX')[1]:][:16])


# ---------------------------------------------------------------- writing
def _number(data, fmt):
    if isinstance(data, bool):
        data = int(data)
    if data is None:
        data = 0
    if fmt in ('<f', '<d'):
        bits = getattr(data, 'bits', None)
        if bits is not None:
            return struct.pack('<I' if fmt == '<f' else '<Q', bits)
        return struct.pack(fmt, float(data))
    if isinstance(data, float):
        data = int(data)
    try:
        return struct.pack(fmt, data)
    except struct.error as e:
        raise OverflowError(f'EBX integer field exceeds its native width: {data}') from e


class _Builder:
    def __init__(self, doc, refuse_new_array_hashes, preserve_layout):
        self.doc = doc
        self.refuse = refuse_new_array_hashes
        self.preserve = preserve_layout
        self.p = bytearray()
        self.instance_offsets, self.pointer_offsets, self.resource_offsets = [], [], []
        self.import_offsets, self.type_info_offsets = [], []
        self.array_slots, self.strings_pending, self.pointers_pending, self.boxes_pending = [], [], [], []
        self.array_data = {}
        self.arrays_out, self.boxed_out = [], []
        self.arrays_offset = self.boxed_offset = self.empty_array_target = 0
        self.old_arrays, self.old_instances, self.carried = {}, {}, {}
        self.boxed_targets = []
        self.known_hashes = None

    def at_least(self, computed, source):
        """With preserve_layout, nothing moves ahead of where the file had it: dead space a tool left
        (ReSkate Studio keeps the room of array elements it removed) stays, so an unchanged
        document writes back byte for byte. What grew simply flows past. The gap keeps the file's
        own bytes (Studio leaves the removed elements' old data there); nothing points into it."""
        if not (self.preserve and source and source > computed):
            return computed
        end = min(source, len(self.doc.payload))
        if end > len(self.p):
            self.p.extend(self.doc.payload[len(self.p):end])
        return source

    def run(self):
        self.validate()
        self.write_instances()
        self.write_arrays()
        self.write_boxed_values()
        string_offset = self.at_least(len(self.p), self.doc.strings_offset)
        self.grow(string_offset)
        self.write_strings()
        self.resolve_pointers()
        return self.write_riff(bytes(self.p), self.write_fixup(string_offset), self.write_extra())

    def validate(self):
        doc = self.doc
        if not doc.reflection_data:
            raise ValueError('EBX has no donor reflection table')
        internal = False
        for inst in doc.instances:
            internal |= not inst.exported
            if internal and inst.exported:
                raise ValueError('exported EBX instances must precede internal instances')
            if not 0 <= inst.descriptor < len(doc.types):
                raise ValueError('EBX instance has no valid type descriptor')
            if inst.fixup_type >= len(doc.fixup_type_guids):
                raise ValueError('EBX instance has no valid fixup type')

    def grow(self, end):
        if end > len(self.p):
            self.p.extend(bytes(end - len(self.p)))

    def put(self, offset, data):
        self.grow(offset + len(data))
        self.p[offset:offset + len(data)] = data

    def u32(self, offset, value):
        self.put(offset, struct.pack('<I', value & 0xFFFFFFFF))

    def write_instances(self):
        cursor = 0
        for inst in self.doc.instances:
            t = self.doc.types[inst.descriptor]
            start = self.at_least(align_up(cursor + (16 if inst.exported else 0), max(t.alignment, 1)), inst.data_offset)
            if inst.exported:
                self.put(start - 16, bytes(inst.instance_guid))
            self.grow(start + t.size)
            if inst.raw_image:
                self.put(start, inst.raw_image[:t.size])
            self.put(start, struct.pack('<H', inst.fixup_type))
            self.write_type(inst.descriptor, start, inst.object)
            self.instance_offsets.append(start)
            cursor = start + t.size

    def write_type(self, type_index, start, obj):
        doc = self.doc
        t = doc.types[type_index]
        self.grow(start + t.size)
        values = {f.descriptor: f.value for f in obj.fields} if obj is not None else {}
        for k in range(t.field_count):
            di = t.field_index + k
            f = doc.fields[di]
            if f.type() == INHERITED:
                self.write_type(f.class_ref, start, obj)
                continue
            pos = start + f.data_offset
            value = values.get(di)
            if f.category() == CAT_ARRAY:
                self.grow(pos + 8)
                self.pointer_offsets.append(pos)
                self.array_slots.append((pos, value))
            elif f.type() == STRUCTURE:
                nested_start = align_up(pos, max(doc.types[f.class_ref].alignment, 1))
                nested = value.data if value is not None and isinstance(value.data, Object) else None
                self.write_type(f.class_ref, nested_start, nested)
            else:
                self.write_scalar(f, pos, value)

    def write_scalar(self, f, pos, value):
        data = value.data if value is not None else None
        t = f.type()
        if t == BOOL:
            self.put(pos, struct.pack('<B', 1 if data else 0))
        elif t in INT_FORMATS:
            self.put(pos, _number(data, INT_FORMATS[t]))
        elif t == FLOAT32:
            self.put(pos, _number(data, '<f'))
        elif t == FLOAT64:
            self.put(pos, _number(data, '<d'))
        elif t == GUID:
            self.put(pos, bytes(data) if isinstance(data, Guid) else bytes(16))
        elif t == SHA1:
            self.put(pos, bytes(data) if isinstance(data, Sha1) else bytes(20))
        elif t == STRING:
            raw = _raw(data) if isinstance(data, str) else b''
            if len(raw) > 31:
                raise OverflowError('EBX fixed string exceeds 31 bytes')
            self.put(pos, raw.ljust(32, b'\x00'))
        elif t in (CSTRING, FILEREF):
            self.put(pos, bytes(8))
            self.pointer_offsets.append(pos)
            self.strings_pending.append((pos, data if isinstance(data, str) else '',
                                         value.source_offset if value is not None else None))
        elif t == RESOURCEREF:
            self.put(pos, struct.pack('<Q', data.id if isinstance(data, ResourceRef) else 0))
            if not (self.preserve and isinstance(data, ResourceRef) and data.listed is False):
                self.resource_offsets.append(pos)
        elif t == POINTER:
            self.put(pos, bytes(8))
            self.pointers_pending.append((pos, data if isinstance(data, Pointer) else Pointer()))
        elif t in (TYPEREF, DELEGATE):
            encoded = data.encoded if isinstance(data, TypeRef) else 0
            self.put(pos, struct.pack('<II', encoded, 0))
            if encoded and t == TYPEREF:
                self.type_info_offsets.append(pos)
        elif t == BOXEDVALUEREF:
            self.grow(pos + 16)
            if not isinstance(data, BoxedRef) or data.encoded_type == 0:
                self.put(pos, bytes(16))
                return
            self.put(pos, struct.pack('<II', data.encoded_type, 0))
            self.type_info_offsets.append(pos)
            self.pointer_offsets.append(pos + 8)
            self.boxes_pending.append((pos, data))
        else:
            self.grow(pos + SCALAR_SIZE.get(t, 8))

    def element_stride(self, f):
        if f.type() == STRUCTURE:
            return max(self.doc.types[f.class_ref].size, 1)
        return SCALAR_SIZE.get(f.type(), 8)

    def element_alignment(self, f):
        if f.type() == STRUCTURE:
            return max(self.doc.types[f.class_ref].alignment, 1)
        return _scalar_alignment(f.type())

    def fixup_slot(self, descriptor):
        doc = self.doc
        if descriptor >= len(doc.types):
            return 0xFFFF
        t = doc.types[descriptor]
        for i in range(min(len(doc.fixup_type_guids), len(doc.fixup_type_signatures))):
            if doc.fixup_type_guids[i] == t.guid and doc.fixup_type_signatures[i] == t.signature:
                return i
        return 0xFFFF

    def array_hash(self, f):
        raw_type = (f.encoded_type << 1) & 0xFFFF
        class_ref = self.fixup_slot(f.class_ref) if f.type() == STRUCTURE else 0xFFFF
        for a in self.doc.arrays:
            if a.encoded_type == raw_type and a.class_ref == class_ref:
                return a.hash
        return self.doc.arrays[0].hash if self.doc.arrays else f.name_hash

    def new_array_hash(self, holder, descriptor, f):
        if self.known_hashes is None:
            self.known_hashes = {}
            for inst in self.doc.instances:
                if inst.object is not None:
                    self._collect(inst.object)
        found = self.known_hashes.get((holder, descriptor))
        if self.refuse:
            if found is None:
                raise ValueError(f'EBX new array {f.name} has no array of its field to take a hash from')
            if self.doc.hashed_reflection_names and all(g[0] != found for g in self.doc.reflection_groups):
                raise ValueError(f"EBX array hash of {f.name} is not in the reflection's group table")
        return found if found is not None else self.array_hash(f)

    def _collect(self, obj):
        for fv in obj.fields:
            if fv.value.array_hash is not None:
                self.known_hashes.setdefault((obj.descriptor, fv.descriptor), fv.value.array_hash)
            self._collect_value(fv.value)

    def _collect_value(self, v):
        if isinstance(v.data, Object):
            self._collect(v.data)
        elif isinstance(v.data, list):
            for e in v.data:
                self._collect_value(e)

    def array_order(self):
        """Every non-empty array, in the order ReSkate's writer emits them: the instances' arrays in
        field order, each followed depth-first by the arrays inside its elements. Arrays read from a
        file then move to their original order (the game's own files place nested arrays first)."""
        seq = []

        def visit(obj):
            for fv in obj.fields:
                v = fv.value
                if self.doc.fields[fv.descriptor].category() == CAT_ARRAY:
                    if isinstance(v.data, list) and v.data:
                        seq.append((v, fv.descriptor, obj.descriptor))
                        for e in v.data:
                            if isinstance(e.data, Object):
                                visit(e.data)
                elif isinstance(v.data, Object):
                    visit(v.data)

        for inst in self.doc.instances:
            if inst.object is not None:
                visit(inst.object)
        known = sorted((a for a in seq if a[0].source_offset is not None), key=lambda a: a[0].source_offset)
        return known + [a for a in seq if a[0].source_offset is None]

    def write_arrays(self):
        self.arrays_offset = self.at_least(align_up(len(self.p), 16), self.doc.arrays_offset)
        self.grow(self.arrays_offset + 32)
        self.empty_array_target = self.arrays_offset + 16
        tree = self.array_order()
        carried = self.boxed_array_records()
        if self.preserve:
            # Arrays from the file, the tree's and the ones only boxed values reach, in file order.
            known = [(a[0].source_offset, 0, a) for a in tree if a[0].source_offset is not None]
            known += [(r.offset, 1, r) for r in carried]
            for _, kind, item in sorted(known, key=lambda k: (k[0], k[1])):
                if kind == 0:
                    self.place_array(*item)
                else:
                    self.carry_array(item)
            for item in tree:
                if item[0].source_offset is None:
                    self.place_array(*item)
        else:
            for item in tree:
                self.place_array(*item)
            for record in carried:
                self.carry_array(record)
        for slot, value in self.array_slots:
            placed = self.array_data.get(id(value)) if value is not None else None
            self.u32(slot, (placed if placed is not None else self.empty_array_target) - slot)

    # What a boxed value points at is not in the document's values: its bytes are carried as they
    # were, with the arrays and strings its pointers reach.
    def old_target(self, slot):
        old = self.doc.payload
        disp = struct.unpack_from('<i', old, slot)[0]
        target = (slot + disp) & 0xFFFFFFFF
        if target in self.old_arrays:
            return 'array', target
        if self.doc.strings_offset <= target < len(old):
            return 'string', target
        if target in self.old_instances:
            return 'instance', self.old_instances[target]
        if self.doc.boxed_values_offset <= target < self.doc.strings_offset:
            return 'boxed', target
        if 4 <= target <= len(old) and struct.unpack_from('<I', old, target - 4)[0] == 0:
            return 'empty', target
        return 'unknown', target

    @staticmethod
    def old_slots(start, length, offsets):
        return [v for v in sorted(set(offsets)) if start <= v < start + length]

    def boxed_array_records(self):
        """The array records boxed values' pointers reach, in the order ReSkate's writer carries them."""
        doc = self.doc
        if not doc.payload:
            return []
        for a in doc.arrays:
            self.old_arrays.setdefault(a.offset, a)
        for i, inst in enumerate(doc.instances):
            self.old_instances.setdefault(inst.data_offset, i)
        out = []
        for source in sorted(doc.boxed_values, key=lambda v: v.offset):
            for slot in self.old_slots(source.offset, len(source.raw_bytes), doc.pointer_offsets):
                kind, key = self.old_target(slot)
                if kind == 'array':
                    out.append(self.old_arrays[key])
        return out

    def array_element(self, record):
        probe = FieldDescriptor()
        probe.encoded_type = record.encoded_type >> 1
        if probe.type() == STRUCTURE:
            slot = record.class_ref
            doc = self.doc
            if slot < len(doc.fixup_type_guids) and slot < len(doc.fixup_type_signatures):
                for t in doc.types:
                    if t.guid == doc.fixup_type_guids[slot] and t.signature == doc.fixup_type_signatures[slot]:
                        return max(t.size, 1), max(t.alignment, 1)
            raise ValueError('EBX array record has an unknown struct type')
        return SCALAR_SIZE.get(probe.type(), 8), _scalar_alignment(probe.type())

    def carry_array(self, record):
        if record.offset in self.carried:
            return
        stride, alignment = self.array_element(record)
        length = stride * record.count
        if record.offset + length > len(self.doc.payload):
            raise ValueError('EBX carried array exceeds EBXD')
        data = self.at_least(align_up(len(self.p) + 4, alignment), record.offset)
        self.u32(data - 4, record.count)
        self.put(data, self.doc.payload[record.offset:record.offset + length])
        self.carried[record.offset] = data
        self.arrays_out.append(ArrayRecord(data, record.count, record.hash, record.encoded_type, record.class_ref))
        self.relocate(record.offset, length, data)

    def relocate(self, old_start, length, new_start):
        doc = self.doc
        moved = lambda old: new_start + (old - old_start)
        for old in self.old_slots(old_start, length, doc.resource_ref_offsets):
            self.resource_offsets.append(moved(old))
        for old in self.old_slots(old_start, length, doc.import_offsets):
            self.import_offsets.append(moved(old))
        for old in self.old_slots(old_start, length, doc.type_info_offsets):
            self.type_info_offsets.append(moved(old))
        for old in self.old_slots(old_start, length, doc.pointer_offsets):
            slot = moved(old)
            self.pointer_offsets.append(slot)
            if not doc.payload:
                continue
            kind, key = self.old_target(old)
            if kind == 'array':
                self.carry_array(self.old_arrays[key])
                self.u32(slot, self.carried[key] - slot)
            elif kind == 'string':
                end = doc.payload.index(b'\x00', key) if b'\x00' in doc.payload[key:] else len(doc.payload)
                self.strings_pending.append((slot, _text(doc.payload[key:end]), key))
            elif kind == 'instance':
                self.u32(slot, self.instance_offsets[key] - slot)
            elif kind == 'empty':
                self.u32(slot, self.empty_array_target - slot)
            elif kind == 'boxed':
                self.boxed_targets.append((slot, key))

    def place_array(self, array, descriptor, holder):
        values = array.data
        f = self.doc.fields[descriptor]
        # The game's own layout aligns the count word, then the data, to the element alignment;
        # ReSkate's (and Studio's) writer aligns only the data. An array read from a file goes back
        # where it was whenever that is a valid place; a new or moved one follows the game's rule.
        alignment = self.element_alignment(f)
        minimum = align_up(len(self.p) + 4, alignment)
        source = array.source_offset
        if self.preserve and source is not None and source >= minimum:
            data = self.at_least(minimum, source)
        else:
            data = align_up(align_up(len(self.p), alignment) + 4, alignment)
        stride = self.element_stride(f)
        self.u32(data - 4, len(values))
        self.grow(data + stride * len(values))
        for e, value in enumerate(values):
            start = data + e * stride
            if f.type() == STRUCTURE:
                self.write_type(f.class_ref, start, value.data if isinstance(value.data, Object) else None)
            else:
                self.write_scalar(f, start, value)
        h = array.array_hash
        if h is None:
            h = self.new_array_hash(holder, descriptor, f)
        # Struct and enum arrays name their type's fixup slot (the game's files do it for enums too).
        class_ref = self.fixup_slot(f.class_ref) if f.type() in (STRUCTURE, ENUM) else 0xFFFF
        self.arrays_out.append(ArrayRecord(data, len(values), h, (f.encoded_type << 1) & 0xFFFF, class_ref))
        self.array_data[id(array)] = data

    def write_boxed_values(self):
        doc = self.doc
        self.boxed_offset = self.at_least(align_up(len(self.p), 16), doc.boxed_values_offset)
        self.grow(self.boxed_offset)
        moved = {}
        for source in sorted(doc.boxed_values, key=lambda v: v.offset):
            if source.offset < doc.boxed_values_offset:
                raise ValueError('EBX boxed record precedes its section')
            offset = self.boxed_offset + source.offset - doc.boxed_values_offset
            self.put(offset, source.raw_bytes)
            self.relocate(source.offset, len(source.raw_bytes), offset)
            moved[source.offset] = offset
            self.boxed_out.append(BoxedValueRecord(offset, source.count, source.hash, source.encoded_type,
                                                   source.class_ref, source.raw_bytes))
        for slot, target in self.boxed_targets:
            self.u32(slot, self.boxed_offset + (target - doc.boxed_values_offset) - slot)
        for slot, ref in self.boxes_pending:
            self.put(slot, struct.pack('<II', ref.encoded_type, 0))
            relative = moved[ref.data_offset] - (slot + 8) if ref.data_offset in moved else ref.relative_offset
            self.put(slot + 8, struct.pack('<q', relative))

    def write_strings(self):
        # ReSkate's writer keeps one copy of each text. Texts read from a file keep their original
        # order and copies (some tools write a string per field, even empty ones); new texts share
        # an existing copy when there is one, else follow in the order they were met.
        rank, keys = {}, []
        for index, (_, value, source) in enumerate(self.strings_pending):
            if self.preserve and source is not None:
                key = ('file', source, value)
                rank.setdefault(key, (0, source, index))
            else:
                key = None
            keys.append(key)
        by_text = {}
        for key in sorted(rank, key=rank.get):
            by_text.setdefault(key[2], key)
        for index, (_, value, _) in enumerate(self.strings_pending):
            if keys[index] is None:
                key = by_text.get(value)
                if key is None:
                    key = by_text[value] = ('new', value)
                    rank[key] = (1, 0, index)
                keys[index] = key
        offsets = {}
        placed = []   # (file offset, its bytes, new offset) of the file's texts written so far
        for key in sorted(rank, key=rank.get):
            raw = _raw(key[-1]) + b'\x00'
            if key[0] == 'file':
                # A text the file kept inside another (a shared tail, an empty string on a
                # terminator) points into that text's new copy again.
                source = key[1]
                host = next((h for h in reversed(placed) if h[0] <= source and source + len(raw) <= h[0] + len(h[1])
                             and h[1][source - h[0]:source - h[0] + len(raw)] == raw), None)
                if host is not None:
                    offsets[key] = host[2] + source - host[0]
                    continue
                # No earlier than it sat: text a tool orphaned in the table keeps its room.
                self.at_least(len(self.p), source)
                placed.append((source, raw, len(self.p)))
            offsets[key] = len(self.p)
            self.p.extend(raw)
        for (slot, _, _), key in zip(self.strings_pending, keys):
            self.u32(slot, offsets[key] - slot)
            self.u32(slot + 4, 0)
        # Some tools pad the string table (to keep every file one size); that tail stays too.
        self.at_least(len(self.p), self.doc.data_end - self.doc.data_start)

    def resolve_pointers(self):
        for slot, ptr in self.pointers_pending:
            if ptr.kind == 'internal':
                if not 0 <= ptr.index < len(self.instance_offsets):
                    raise ValueError('EBX internal pointer index is invalid')
                self.u32(slot, self.instance_offsets[ptr.index] - slot)
                self.pointer_offsets.append(slot)
            elif ptr.kind == 'external':
                if not 0 <= ptr.index < len(self.doc.imports):
                    raise ValueError('EBX import pointer index is invalid')
                self.u32(slot, (ptr.index << 1) | 1)
                self.import_offsets.append(slot)
            self.u32(slot + 4, 0)

    @staticmethod
    def table(values):
        values = sorted(set(values))
        return struct.pack(f'<I{len(values)}I', len(values), *values)

    def write_fixup(self, string_offset):
        doc = self.doc
        out = bytearray(bytes(doc.file_guid))
        out += struct.pack('<I', len(doc.fixup_type_guids)) + b''.join(bytes(g) for g in doc.fixup_type_guids)
        out += struct.pack(f'<I{len(doc.fixup_type_signatures)}I', len(doc.fixup_type_signatures), *doc.fixup_type_signatures)
        out += struct.pack('<I', sum(1 for i in doc.instances if i.exported))
        out += self.table(self.instance_offsets)
        out += self.table(self.pointer_offsets)
        out += self.table(self.resource_offsets)
        out += struct.pack('<I', len(doc.imports)) + b''.join(bytes(f) + bytes(c) for f, c in doc.imports)
        out += self.table(self.import_offsets)
        out += self.table(self.type_info_offsets)
        out += struct.pack('<III', self.arrays_offset, self.boxed_offset, string_offset)
        old_slots = sorted(doc.boxed_value_slots)
        new_slots = sorted(slot for slot, _ in self.boxes_pending)
        trailing = []
        for old in doc.boxed_reference_offsets:
            if old in old_slots:
                index = old_slots.index(old)
                if index < len(new_slots):
                    trailing.append(new_slots[index])
        if getattr(doc, '_has_boxed_table', True) or trailing:
            out += self.table(trailing)
        return bytes(out)

    def write_extra(self):
        out = bytearray(struct.pack('<II', len(self.arrays_out), len(self.boxed_out)))
        for v in sorted(self.arrays_out, key=lambda a: a.offset):
            out += struct.pack('<IIIHH', v.offset, v.count, v.hash, v.encoded_type, v.class_ref)
        for v in sorted(self.boxed_out, key=lambda a: a.offset):
            out += struct.pack('<IIIHH', v.offset, v.count, v.hash, v.encoded_type, v.class_ref)
        return bytes(out)

    def write_riff(self, payload, fixup, extra):
        doc = self.doc
        chunks = {b'EBXD': payload, b'EFIX': fixup, b'EBXX': extra,
                  (b'RFL2' if doc.hashed_reflection_names else b'REFL'): doc.reflection_data}
        order = [c for c in getattr(doc, '_chunk_order', []) if c in chunks]
        order += [c for c in (b'EBXD', b'EFIX', b'EBXX', b'RFL2', b'REFL') if c in chunks and c not in order]
        out = bytearray(RIFF + bytes(4) + (EBXS if doc.serialized_layout else EBX))
        for cid in order:
            body = chunks[cid]
            if cid == b'EBXD':  # the data starts 16-aligned (12 zero bytes when EBXD comes first)
                body = bytes(-(len(out) + 8) % 16) + body
            out += cid + struct.pack('<I', len(body)) + body
            if len(body) & 1:
                out += b'\x00'
        struct.pack_into('<I', out, 4, len(out) - 8)
        return bytes(out)


def ensure_fixup_type(doc, descriptor):
    """The fixup type slot of a type, added when the document has none for it."""
    t = doc.types[descriptor]
    n = min(len(doc.fixup_type_guids), len(doc.fixup_type_signatures))
    for i in range(n):
        if doc.fixup_type_guids[i] == t.guid and doc.fixup_type_signatures[i] == t.signature:
            return i
    doc.fixup_type_guids.insert(n, t.guid)
    doc.fixup_type_signatures.append(t.signature)
    return n


def write_document(doc, refuse_new_array_hashes=False, preserve_layout=True):
    """The document as EBX bytes. preserve_layout keeps instances, arrays and sections no earlier
    than the file had them (byte-identical for an unchanged document); False packs them tight."""
    return _Builder(doc, refuse_new_array_hashes, preserve_layout).run()
