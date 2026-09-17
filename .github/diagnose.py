import ctypes
import json
import msvcrt
import os
from pathlib import Path
import shutil
import subprocess
import sys
from ctypes import wintypes as w

class Entry(ctypes.Structure):
    _fields_ = [("size", w.DWORD), ("usage", w.DWORD), ("pid", w.DWORD), ("heap", ctypes.c_size_t), ("module", w.DWORD), ("threads", w.DWORD), ("parent", w.DWORD), ("priority", w.LONG), ("flags", w.DWORD), ("name", w.WCHAR * 260)]

kernel = ctypes.WinDLL("kernel32", use_last_error=True)
kernel.CreateToolhelp32Snapshot.argtypes = [w.DWORD, w.DWORD]
kernel.CreateToolhelp32Snapshot.restype = w.HANDLE
kernel.Process32FirstW.argtypes = [w.HANDLE, ctypes.POINTER(Entry)]
kernel.Process32NextW.argtypes = kernel.Process32FirstW.argtypes
kernel.OpenProcess.argtypes = [w.DWORD, w.BOOL, w.DWORD]
kernel.OpenProcess.restype = w.HANDLE
kernel.CloseHandle.argtypes = [w.HANDLE]
kernel.QueryFullProcessImageNameW.argtypes = [w.HANDLE, w.DWORD, w.LPWSTR, ctypes.POINTER(w.DWORD)]
dbghelp = ctypes.WinDLL("dbghelp", use_last_error=True)
dbghelp.MiniDumpWriteDump.argtypes = [w.HANDLE, w.DWORD, w.HANDLE, w.DWORD, w.LPVOID, w.LPVOID, w.LPVOID]
dbghelp.MiniDumpWriteDump.restype = w.BOOL

output = Path(os.environ["RUNNER_TEMP"]) / "diagnostic"
output.mkdir(exist_ok=True)
print("COMMAND", sys.argv[1:], flush=True)
process = subprocess.Popen(sys.argv[1:])
try:
    code = process.wait(timeout=240)
except subprocess.TimeoutExpired:
    snapshot = kernel.CreateToolhelp32Snapshot(2, 0)
    entry = Entry()
    entry.size = ctypes.sizeof(entry)
    records = {}
    try:
        found = kernel.Process32FirstW(snapshot, ctypes.byref(entry))
        while found:
            records[entry.pid] = {"pid": entry.pid, "parent": entry.parent, "name": entry.name}
            found = kernel.Process32NextW(snapshot, ctypes.byref(entry))
    finally:
        kernel.CloseHandle(snapshot)
    selected = {process.pid}
    while additions := {pid for pid, record in records.items() if record["parent"] in selected} - selected:
        selected.update(additions)
    for pid in selected:
        record = records.get(pid, {"pid": pid})
        handle = kernel.OpenProcess(0x0410, False, pid)
        if not handle:
            record["open_error"] = ctypes.get_last_error()
            continue
        try:
            directory = output / str(pid)
            directory.mkdir(exist_ok=True)
            buffer = ctypes.create_unicode_buffer(32768)
            size = w.DWORD(len(buffer))
            if kernel.QueryFullProcessImageNameW(handle, 0, buffer, ctypes.byref(size)):
                image = Path(buffer.value)
                record["image"] = str(image)
                if Path.cwd() in image.parents:
                    shutil.copy2(image, directory / image.name)
                    for symbols in image.parent.glob("*.pdb"):
                        shutil.copy2(symbols, directory / symbols.name)
            with (directory / "process.dmp").open("wb") as dump:
                if not dbghelp.MiniDumpWriteDump(handle, pid, msvcrt.get_osfhandle(dump.fileno()), 0x1002, None, None, None):
                    record["dump_error"] = ctypes.get_last_error()
        finally:
            kernel.CloseHandle(handle)
    (output / "processes.json").write_text(json.dumps(records, indent=2))
    print("CAPTURED", json.dumps([records[pid] for pid in selected if pid in records]), flush=True)
    subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"])
    code = 1
print("EXIT", code, flush=True)
sys.exit(code)
