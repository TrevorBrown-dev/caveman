"""Copies the icon of one Windows program into another, with the Windows resource-update API.
Usage: set_icon.py <source exe> <target exe>   (Caveman.exe takes ReSkateLauncher.exe's icon)"""
import ctypes
import struct
import sys
from ctypes import wintypes

k32 = ctypes.WinDLL('kernel32', use_last_error=True)
LOAD_LIBRARY_AS_DATAFILE = 0x2
RT_ICON, RT_GROUP_ICON = 3, 14

k32.LoadLibraryExW.restype = wintypes.HMODULE
k32.LoadLibraryExW.argtypes = [wintypes.LPCWSTR, wintypes.HANDLE, wintypes.DWORD]
k32.FindResourceW.restype = wintypes.HRSRC
k32.FindResourceW.argtypes = [wintypes.HMODULE, wintypes.LPVOID, wintypes.LPVOID]
k32.LoadResource.restype = wintypes.HGLOBAL
k32.LoadResource.argtypes = [wintypes.HMODULE, wintypes.HRSRC]
k32.LockResource.restype = ctypes.c_void_p
k32.LockResource.argtypes = [wintypes.HGLOBAL]
k32.SizeofResource.restype = wintypes.DWORD
k32.SizeofResource.argtypes = [wintypes.HMODULE, wintypes.HRSRC]
k32.FreeLibrary.argtypes = [wintypes.HMODULE]
k32.BeginUpdateResourceW.restype = wintypes.HANDLE
k32.BeginUpdateResourceW.argtypes = [wintypes.LPCWSTR, wintypes.BOOL]
k32.UpdateResourceW.restype = wintypes.BOOL
k32.UpdateResourceW.argtypes = [wintypes.HANDLE, wintypes.LPVOID, wintypes.LPVOID, wintypes.WORD, wintypes.LPVOID, wintypes.DWORD]
k32.EndUpdateResourceW.restype = wintypes.BOOL
k32.EndUpdateResourceW.argtypes = [wintypes.HANDLE, wintypes.BOOL]
EnumNamesProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HMODULE, wintypes.LPVOID, wintypes.LPVOID, wintypes.LPARAM)
k32.EnumResourceNamesW.argtypes = [wintypes.HMODULE, wintypes.LPVOID, EnumNamesProc, wintypes.LPARAM]


def res_id(n):
    return ctypes.c_void_p(n)


def read_resource(module, rtype, name):
    h = k32.FindResourceW(module, name, res_id(rtype))
    if not h:
        raise OSError(f'resource {name} of type {rtype} not found')
    size = k32.SizeofResource(module, h)
    return ctypes.string_at(k32.LockResource(k32.LoadResource(module, h)), size)


def main(source, target):
    module = k32.LoadLibraryExW(source, None, LOAD_LIBRARY_AS_DATAFILE)
    if not module:
        raise ctypes.WinError(ctypes.get_last_error())
    groups = []
    k32.EnumResourceNamesW(module, res_id(RT_GROUP_ICON),
                           EnumNamesProc(lambda m, t, name, p: groups.append(name) or True), 0)
    if not groups:
        raise SystemExit(f'{source} has no icon')
    first = groups[0]                    # the first icon group is the one Explorer shows
    group = read_resource(module, RT_GROUP_ICON, first)
    count = struct.unpack_from('<H', group, 4)[0]
    icons = []
    for i in range(count):           # GRPICONDIRENTRY: 14 bytes, the icon's resource id last
        icon_id = struct.unpack_from('<H', group, 6 + 14 * i + 12)[0]
        icons.append((icon_id, read_resource(module, RT_ICON, res_id(icon_id))))
    k32.FreeLibrary(module)

    update = k32.BeginUpdateResourceW(target, False)
    if not update:
        raise ctypes.WinError(ctypes.get_last_error())
    for icon_id, data in icons:
        buf = ctypes.create_string_buffer(data, len(data))
        if not k32.UpdateResourceW(update, res_id(RT_ICON), res_id(icon_id), 0, buf, len(data)):
            raise ctypes.WinError(ctypes.get_last_error())
    gbuf = ctypes.create_string_buffer(group, len(group))
    if not k32.UpdateResourceW(update, res_id(RT_GROUP_ICON), res_id(1), 0, gbuf, len(group)):
        raise ctypes.WinError(ctypes.get_last_error())
    if not k32.EndUpdateResourceW(update, False):
        raise ctypes.WinError(ctypes.get_last_error())
    print(f'copied an icon of {count} image(s) from {source} into {target}')


if __name__ == '__main__':
    main(sys.argv[1], sys.argv[2])
