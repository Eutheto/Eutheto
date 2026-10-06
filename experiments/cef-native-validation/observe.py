"""Private, bounded native observations; never serialize this module's process records.

Source contracts: Linux proc_pid_stat(5), proc_pid_status(5), pidfd_open(2),
pidfd_send_signal(2); Microsoft Learn NtQueryInformationProcess/PEB/
RTL_USER_PROCESS_PARAMETERS, GetProcessTimes, QueryFullProcessImageNameW,
GetTokenInformation, IsProcessInJob; Apple xnu sys/proc_info{,_private}.h,
libsyscall/wrappers/libproc and Chromium sandbox/mac/seatbelt.cc.
https://man7.org/linux/man-pages/man2/pidfd_send_signal.2.html
https://learn.microsoft.com/en-us/windows/win32/api/winternl/nf-winternl-ntqueryinformationprocess
https://learn.microsoft.com/en-us/windows/win32/api/winnt/ne-winnt-token_information_class
https://raw.githubusercontent.com/apple-oss-distributions/xnu/43a90889846e00bfb5cf1d255cdc0a701a1e05a4/bsd/kern/proc_info.c
https://raw.githubusercontent.com/apple-oss-distributions/xnu/43a90889846e00bfb5cf1d255cdc0a701a1e05a4/bsd/sys/proc_info_private.h
https://raw.githubusercontent.com/apple-oss-distributions/xnu/43a90889846e00bfb5cf1d255cdc0a701a1e05a4/libsyscall/wrappers/libproc/libproc.c
https://raw.githubusercontent.com/chromium/chromium/b859317bf11f6be47f9b7799ec690a0a42a1fb33/sandbox/mac/seatbelt.cc
https://raw.githubusercontent.com/chromium/chromium/b859317bf11f6be47f9b7799ec690a0a42a1fb33/content/browser/renderer_host/spare_render_process_host_manager_impl.cc

Snapshots are point-in-time descendant observations, not lifetime containment or
an exhaustive post-compromise sandbox audit. An observer deadline bounds loops,
not an individual synchronous kernel call. Permission errors fail closed.
"""
from __future__ import annotations

import ctypes as C
import errno
import os
from pathlib import Path
import re
import signal
import select
import struct
import sys
import time

MAX_SCAN = 8192
MAX_OWNED = 128
MAX_STRING = 32768
MAX_PATH = 4096
MAX_NATIVE = 65536
SCAN_SECONDS = 8.0


class ObservationError(RuntimeError):
    def __init__(self, code: str = "native-observation-unavailable", *,
                 operation: str | None = None, native_error: str | None = None,
                 native_result: str | None = None):
        super().__init__(code)
        self.operation = operation
        self.native_error = native_error
        self.native_result = native_result


def _mac_failure(operation: str, native_errno: int, native_result: str,
                 code: str = "native-observation-unavailable") -> ObservationError:
    # Closed diagnostic categories only; no native text or process data retained.
    errors = {0: "none", errno.EPERM: "permission", errno.EACCES: "permission",
              errno.ESRCH: "not-found", errno.EINVAL: "invalid",
              errno.ENOMEM: "size", errno.EOVERFLOW: "size"}
    return ObservationError(code, operation=operation,
                            native_error=errors.get(native_errno, "other"),
                            native_result=native_result)


class _Pidfd:
    """Snapshot-owned reference; dropping the original record closes it."""
    def __init__(self, pid: int):
        self.pid = pid
        self.fd = os.pidfd_open(pid, 0)

    def live(self) -> bool:
        poll = select.poll()
        poll.register(self.fd, select.POLLIN)
        return not poll.poll(0)

    def __del__(self):
        descriptor = getattr(self, "fd", None)
        if descriptor is not None:
            os.close(descriptor)


def _deadline(end: float) -> None:
    if time.monotonic() > end:
        raise ObservationError("observation-deadline")


def _read(path: Path, limit: int) -> bytes:
    with path.open("rb", buffering=0) as stream:
        value = stream.read(limit + 1)
    if len(value) > limit:
        raise ObservationError("observation-size-limit")
    return value


def _role(args: list[str]) -> str:
    if len(args) > 512:
        raise ObservationError("observation-argument-limit")
    types = [arg[7:] for arg in args if arg.startswith("--type=")]
    if len(types) > 1:
        raise ObservationError("ambiguous-process-role")
    if not types:
        return "browser"
    return types[0] if types[0] in {"renderer", "zygote", "gpu-process", "utility"} else "other-helper"


def _record(pid: int, parent: int, start: str, state: str = "live") -> dict:
    return {"pid": pid, "parent_pid": parent, "start_identity": start, "state": state}


def _same(left: dict, right: dict) -> bool:
    guards = (left.get("_pidfd"), right.get("_pidfd"))
    return all(guard is None or guard.live() for guard in guards) and left["pid"] == right["pid"] and left["start_identity"] == right["start_identity"]


def _owned_path(value: str, root: Path) -> str:
    if not value or len(value) > MAX_PATH or "\0" in value:
        raise ObservationError("invalid-owned-executable")
    path = Path(value).resolve(strict=True)
    if not path.is_relative_to(root) or not path.is_file():
        raise ObservationError("unexpected-descendant-executable")
    return str(path)


def _linux_basic(pid: int) -> dict:
    value = _read(Path(f"/proc/{pid}/stat"), MAX_PATH)
    tail = value[value.rindex(b")") + 2:].split()
    if len(tail) < 20:
        raise ObservationError()
    return _record(pid, int(tail[1]), tail[19].decode("ascii"), tail[0].decode("ascii"))


def _linux_pids() -> list[int]:
    pids = []
    with os.scandir("/proc") as entries:
        # Count all directory entries too; a scan cannot consume unlimited names.
        for count, entry in enumerate(entries, 1):
            if count > MAX_SCAN:
                raise ObservationError("process-scan-limit")
            if entry.name.isdecimal():
                pids.append(int(entry.name))
    return pids


def _linux_restrictions(pid: int) -> dict:
    data = _read(Path(f"/proc/{pid}/status"), MAX_NATIVE)
    keys = {b"NoNewPrivs", b"Seccomp", b"Seccomp_filters", b"CapEff", b"Uid", b"Gid"}
    values = {}
    for line in data.splitlines():
        key, separator, value = line.partition(b":")
        if separator and key in keys:
            base = 16 if key == b"CapEff" else 10
            values[key.decode("ascii")] = [int(word, base) for word in value.split()]
    if not {"NoNewPrivs", "Seccomp", "CapEff", "Uid", "Gid"}.issubset(values):
        raise ObservationError()
    namespaces = {}
    for name in ("user", "pid", "mnt", "net", "ipc", "uts"):
        value = os.readlink(f"/proc/{pid}/ns/{name}")
        if len(value) > 128:
            raise ObservationError("observation-size-limit")
        namespaces[name] = value
    return {"status": values, "namespaces": namespaces}


def _linux_details(pid: int) -> tuple[str, list[str]]:
    executable = os.readlink(f"/proc/{pid}/exe")
    # Only called for descendants. Raw arguments never enter returned records.
    data = _read(Path(f"/proc/{pid}/cmdline"), MAX_STRING)
    if not data or not data.endswith(b"\0"):
        raise ObservationError()
    # Chromium can rewrite argv into one space-separated region. Recognize
    # exact space/NUL-delimited type tokens, not guessed shell quoting. _role
    # rejects duplicate/ambiguous types; no raw arguments leave this function.
    types = re.findall(rb"(?:^|[ \0])--type=([^ \0]*)(?=[ \0]|$)", data)
    return executable, ["--type=" + os.fsdecode(value) for value in types]


def _bind(lib, name, result, arguments):
    function = getattr(lib, name)
    function.restype = result
    function.argtypes = arguments
    return function


class _Win:
    """64-bit Windows SDK layouts, loaded only on the Windows target."""
    def __init__(self):
        if C.sizeof(C.c_void_p) != 8:
            raise ObservationError("requires-native-64-bit-python")
        u32, ptr, size = C.c_uint32, C.c_void_p, C.c_size_t
        self.kernel = C.WinDLL("kernel32", use_last_error=True)
        self.security = C.WinDLL("advapi32", use_last_error=True)
        self.nt = C.WinDLL("ntdll", use_last_error=True)
        self.shell = C.WinDLL("shell32", use_last_error=True)
        self.open = _bind(self.kernel, "OpenProcess", ptr, [u32, C.c_int, u32])
        self.close = _bind(self.kernel, "CloseHandle", C.c_int, [ptr])
        self.times = _bind(self.kernel, "GetProcessTimes", C.c_int, [ptr] + [ptr] * 4)
        self.image = _bind(self.kernel, "QueryFullProcessImageNameW", C.c_int, [ptr, u32, ptr, ptr])
        self.query = _bind(self.nt, "NtQueryInformationProcess", C.c_int32, [ptr, u32, ptr, u32, ptr])
        self.memory = _bind(self.kernel, "ReadProcessMemory", C.c_int, [ptr, ptr, ptr, size, ptr])
        self.wait = _bind(self.kernel, "WaitForSingleObject", u32, [ptr, u32])
        self.kill = _bind(self.kernel, "TerminateProcess", C.c_int, [ptr, u32])
        self.in_job = _bind(self.kernel, "IsProcessInJob", C.c_int, [ptr, ptr, ptr])
        self.token_open = _bind(self.security, "OpenProcessToken", C.c_int, [ptr, u32, ptr])
        self.token_info = _bind(self.security, "GetTokenInformation", C.c_int, [ptr, u32, ptr, u32, ptr])
        self.split = _bind(self.shell, "CommandLineToArgvW", ptr, [C.c_wchar_p, ptr])
        self.free = _bind(self.kernel, "LocalFree", ptr, [ptr])
        self.toolhelp = _bind(self.kernel, "CreateToolhelp32Snapshot", ptr, [u32, u32])
        self.first = _bind(self.kernel, "Process32FirstW", C.c_int, [ptr, ptr])
        self.next = _bind(self.kernel, "Process32NextW", C.c_int, [ptr, ptr])

    def handle(self, pid: int, access: int = 0x1000):
        handle = self.open(access, False, pid)
        if not handle:
            error = C.get_last_error()
            if error == 87:  # ERROR_INVALID_PARAMETER: PID no longer exists.
                raise ProcessLookupError()
            raise ObservationError()
        return handle

    def basic_handle(self, pid: int, handle) -> dict:
        times = [C.c_uint64() for _ in range(4)]
        if not self.times(handle, *(C.byref(value) for value in times)):
            raise ObservationError()
        wait = self.wait(handle, 0)
        # Handles without SYNCHRONIZE cannot be waited on; creation time is still
        # usable, and basic() requests SYNCHRONIZE explicitly.
        if wait not in (0, 258):
            raise ObservationError()
        return _record(pid, 0, str(times[0].value), "exited" if wait == 0 else "live")

    def basic(self, pid: int) -> dict:
        handle = self.handle(pid, 0x1000 | 0x100000)
        try:
            return self.basic_handle(pid, handle)
        finally:
            self.close(handle)

    def topology(self) -> dict[int, int]:
        class Entry(C.Structure):
            _fields_ = [("size", C.c_uint32), ("usage", C.c_uint32),
                        ("pid", C.c_uint32), ("heap", C.c_size_t),
                        ("module", C.c_uint32), ("threads", C.c_uint32),
                        ("parent", C.c_uint32), ("priority", C.c_int32),
                        ("flags", C.c_uint32), ("exe", C.c_wchar * 260)]
        handle = self.toolhelp(2, 0)
        if handle == C.c_void_p(-1).value:
            raise ObservationError()
        try:
            entry = Entry()
            entry.size = C.sizeof(entry)
            result = {}
            ok = self.first(handle, C.byref(entry))
            if not ok:
                raise ObservationError()
            while ok:
                if len(result) >= MAX_SCAN:
                    raise ObservationError("process-scan-limit")
                result[entry.pid] = entry.parent
                ok = self.next(handle, C.byref(entry))
            if C.get_last_error() != 18:  # ERROR_NO_MORE_FILES
                raise ObservationError()
            return result
        finally:
            self.close(handle)

    def remote(self, handle, address: int, count: int) -> bytes:
        if not address or not 0 < count <= MAX_NATIVE:
            raise ObservationError()
        data = C.create_string_buffer(count)
        got = C.c_size_t()
        if not self.memory(handle, address, data, count, C.byref(got)) or got.value != count:
            raise ObservationError()
        return data.raw

    def details(self, pid: int) -> tuple[str, list[str], int]:
        class Basic(C.Structure):
            _fields_ = [("exit", C.c_int32), ("peb", C.c_void_p),
                        ("affinity", C.c_size_t), ("priority", C.c_int32),
                        ("pid", C.c_size_t), ("parent", C.c_size_t)]
        handle = self.handle(pid, 0x400 | 0x10)
        try:
            image = C.create_unicode_buffer(MAX_PATH)
            length = C.c_uint32(MAX_PATH)
            if not self.image(handle, 0, image, C.byref(length)) or length.value >= MAX_PATH:
                raise ObservationError()
            basic = Basic()
            got = C.c_uint32()
            if self.query(handle, 0, C.byref(basic), C.sizeof(basic), C.byref(got)) < 0:
                raise ObservationError()
            if got.value != C.sizeof(basic) or basic.pid != pid:
                raise ObservationError()
            wow64 = C.c_size_t()
            if self.query(handle, 26, C.byref(wow64), C.sizeof(wow64), C.byref(got)) < 0 or wow64.value:
                raise ObservationError("requires-native-64-bit-process")
            # Published 64-bit PEB prefix: ProcessParameters at byte32.
            peb = self.remote(handle, basic.peb, 40)
            parameters = struct.unpack_from("<Q", peb, 32)[0]
            # RTL_USER_PROCESS_PARAMETERS: Reserved1[16], Reserved2[10],
            # ImagePathName UNICODE_STRING, CommandLine UNICODE_STRING.
            params = self.remote(handle, parameters, 128)
            length, maximum, address = struct.unpack_from("<HH4xQ", params, 112)
            if length > MAX_STRING or length > maximum or length % 2 or not length:
                raise ObservationError("invalid-native-commandline")
            command = self.remote(handle, address, length).decode("utf-16-le", errors="strict")
            if "\0" in command:
                raise ObservationError("invalid-native-commandline")
            count = C.c_int()
            argv = self.split(command, C.byref(count))
            if not argv:
                raise ObservationError()
            try:
                if not 0 < count.value <= 512:
                    raise ObservationError("observation-argument-limit")
                array = C.cast(argv, C.POINTER(C.c_wchar_p))
                args = [array[index] for index in range(count.value)]
            finally:
                self.free(argv)
            return image.value, args, int(basic.parent)
        finally:
            self.close(handle)

    def token(self, handle) -> dict:
        token = C.c_void_p()
        if not self.token_open(handle, 8, C.byref(token)):
            raise ObservationError()
        try:
            def information(kind: int):
                needed = C.c_uint32()
                C.set_last_error(0)
                if self.token_info(token, kind, None, 0, C.byref(needed)) or C.get_last_error() != 122:
                    raise ObservationError()
                if not 0 < needed.value <= MAX_NATIVE:
                    raise ObservationError("observation-size-limit")
                buffer = C.create_string_buffer(needed.value)
                if not self.token_info(token, kind, buffer, len(buffer), C.byref(needed)) or needed.value > len(buffer):
                    raise ObservationError()
                return buffer, needed.value

            def scalar(kind: int) -> int:
                buffer, length = information(kind)
                if length != 4:
                    raise ObservationError()
                return struct.unpack_from("<I", buffer.raw)[0]

            label, length = information(25)  # TOKEN_MANDATORY_LABEL
            if length < 16:
                raise ObservationError()
            sid = struct.unpack_from("<Q", label.raw)[0]
            offset = sid - C.addressof(label)
            if not 0 <= offset <= length - 12:
                raise ObservationError()
            revision, count = struct.unpack_from("BB", label.raw, offset)
            if revision != 1 or count != 1 or label.raw[offset + 2:offset + 8] != b"\0\0\0\0\0\x10":
                raise ObservationError("invalid-integrity-sid")
            integrity = struct.unpack_from("<I", label.raw, offset + 8)[0]
            groups, group_length = information(11)  # TOKEN_GROUPS / TokenRestrictedSids
            if group_length < 4:
                raise ObservationError()
            restricted_count = struct.unpack_from("<I", groups.raw)[0]
            if restricted_count > 4096 or (restricted_count and 8 + 16 * restricted_count > group_length):
                raise ObservationError()
            privileges, privilege_length = information(3)
            if privilege_length < 4:
                raise ObservationError()
            count = struct.unpack_from("<I", privileges.raw)[0]
            if count > 4096 or 4 + 12 * count > privilege_length:
                raise ObservationError()
            enabled = sum(bool(struct.unpack_from("<I", privileges.raw, 4 + 12 * index + 8)[0] & 2)
                          for index in range(count))
            job = C.c_int()
            if not self.in_job(handle, None, C.byref(job)):
                raise ObservationError()
            return {"integrity_rid": integrity, "restricted_sid_count": restricted_count,
                    "has_ever_been_filtered": bool(scalar(21)),
                    "appcontainer": bool(scalar(29)), "lpac": bool(scalar(46)),
                    "privilege_count": count, "enabled_privilege_count": enabled,
                    "in_any_job": bool(job.value)}
        finally:
            self.close(token)


class _Mac:
    def __init__(self):
        self.proc = C.CDLL("/usr/lib/libproc.dylib", use_errno=True)
        self.system = C.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
        self.sandbox = C.CDLL("/usr/lib/libsandbox.dylib", use_errno=True)
        self.info = _bind(self.proc, "proc_pidinfo", C.c_int,
                          [C.c_int, C.c_int, C.c_uint64, C.c_void_p, C.c_int])
        self.list = _bind(self.proc, "proc_listpids", C.c_int,
                          [C.c_uint32, C.c_uint32, C.c_void_p, C.c_int])
        self.path = _bind(self.proc, "proc_pidpath", C.c_int, [C.c_int, C.c_void_p, C.c_uint32])
        self.sysctl = _bind(self.system, "sysctl", C.c_int,
                            [C.c_void_p, C.c_uint, C.c_void_p, C.c_void_p, C.c_void_p, C.c_size_t])
        # Established seatbelt query used by Chromium Seatbelt::IsSandboxed.
        self.check = _bind(self.sandbox, "sandbox_check", C.c_int,
                           [C.c_int, C.c_char_p, C.c_int])

    def basic(self, pid: int) -> dict:
        # proc_bsdinfo is 136 bytes; proc_uniqidentifierinfo is 56 bytes.
        # Flavor18 returns both atomically, avoiding mismatched identities.
        data = C.create_string_buffer(192)
        C.set_errno(0)
        count = self.info(pid, 18, 0, data, len(data))
        native_errno = C.get_errno()
        if count != len(data):
            failure = _mac_failure("identity", native_errno,
                                   "zero" if count <= 0 else "short" if count < len(data) else "oversized")
            if native_errno == errno.ESRCH:
                missing = ProcessLookupError()
                missing.operation = failure.operation
                missing.native_error = failure.native_error
                missing.native_result = failure.native_result
                raise missing
            raise failure
        flags, state, _, actual, parent = struct.unpack_from("<5I", data.raw)
        if actual != pid:
            raise _mac_failure("identity", 0, "ok")
        sec, usec = struct.unpack_from("<QQ", data.raw, 120)
        unique, parent_unique, version = struct.unpack_from("<QQI", data.raw, 152)
        record = _record(pid, parent, f"{unique}:{version}:{sec}:{usec}", "zombie" if state == 5 else "live")
        record["_unique"] = unique
        record["_parent_unique"] = parent_unique
        record["_pidversion"] = version
        return record

    def topology(self, end: float) -> dict[int, int]:
        data = (C.c_int * (MAX_SCAN + 1))()
        C.set_errno(0)
        count = self.list(1, 0, data, C.sizeof(data))
        native_errno = C.get_errno()
        if count <= 0 or count % 4 or count >= C.sizeof(data):
            raise _mac_failure("topology", native_errno,
                               "zero" if count <= 0 else "oversized" if count >= C.sizeof(data) else "short",
                               "process-scan-limit")
        result = {}
        for pid in data[:count // 4]:
            _deadline(end)
            if pid > 0:
                # SHORTBSDINFO has no same-user gate: topology enumeration
                # never requests full identity/arguments of unrelated users.
                short = C.create_string_buffer(64)
                C.set_errno(0)
                size = self.info(pid, 13, 0, short, len(short))
                native_errno = C.get_errno()
                if size != len(short):
                    if native_errno == errno.ESRCH:
                        continue
                    raise _mac_failure("short-info", native_errno,
                                       "zero" if size <= 0 else "short" if size < len(short) else "oversized")
                actual, parent = struct.unpack_from("<II", short.raw)
                if actual != pid:
                    raise _mac_failure("short-info", 0, "ok")
                result[pid] = parent
        return result

    def details(self, pid: int) -> tuple[str, list[str]]:
        image = C.create_string_buffer(MAX_PATH)
        C.set_errno(0)
        length = self.path(pid, image, len(image))
        native_errno = C.get_errno()
        if not 0 < length < MAX_PATH:
            raise _mac_failure("image", native_errno, "zero" if length <= 0 else "oversized")
        # CTL_KERN=1, KERN_PROCARGS2=49; fixed output, no environment retained.
        mib = (C.c_int * 3)(1, 49, pid)
        data = C.create_string_buffer(MAX_STRING)
        size = C.c_size_t(len(data))
        C.set_errno(0)
        status = self.sysctl(mib, 3, data, C.byref(size), None, 0)
        native_errno = C.get_errno()
        if status != 0 or not 4 < size.value <= len(data):
            raise _mac_failure("args", native_errno,
                               "oversized" if size.value > len(data) else "zero" if status != 0 or size.value == 0 else "short")
        raw = data.raw[:size.value]
        argc = struct.unpack_from("<i", raw)[0]
        if not 0 < argc <= 512:
            raise ObservationError("observation-argument-limit")
        cursor = raw.index(b"\0", 4) + 1  # skip executable string and padding
        while cursor < len(raw) and raw[cursor] == 0:
            cursor += 1
        args = []
        for _ in range(argc):
            stop = raw.index(b"\0", cursor)
            args.append(os.fsdecode(raw[cursor:stop]))
            cursor = stop + 1
        return os.fsdecode(image.raw[:length]), args

    def terminate(self, process: dict) -> bool:
        # The kernel selects by PID + pidversion, checks ordinary signal
        # permissions and retains the proc during delivery. No PID-only kill.
        # proc_terminate[_with_audittoken] chooses SIGTERM for untracked/dirty
        # processes; its output signal is not an input SIGKILL request.
        send = _bind(self.proc, "proc_signal_with_audittoken", C.c_int, [C.c_void_p, C.c_int])
        token = (C.c_uint32 * 8)()
        token[5] = process["pid"]
        token[7] = process["_pidversion"]
        return send(token, signal.SIGKILL) == 0


_backend_instance = None


def _backend():
    global _backend_instance
    if _backend_instance is None:
        if sys.platform == "win32":
            _backend_instance = _Win()
        elif sys.platform == "darwin":
            _backend_instance = _Mac()
        elif not sys.platform.startswith("linux"):
            raise ObservationError("unsupported-observation-platform")
    return _backend_instance


def _basic(pid: int) -> dict:
    native = _backend()
    return native.basic(pid) if native else _linux_basic(pid)


def _validate_records(processes: list[dict]) -> None:
    if not isinstance(processes, list) or not 0 < len(processes) <= MAX_OWNED:
        raise ObservationError("invalid-process-observations")
    seen = set()
    for process in processes:
        pid = process.get("pid")
        identity = process.get("start_identity")
        if type(pid) is not int or not 0 < pid <= 0x7fffffff or pid in seen or not isinstance(identity, str) or not 0 < len(identity) <= 128:
            raise ObservationError("invalid-process-observations")
        seen.add(pid)
        if process.get("_platform") != sys.platform:
            raise ObservationError("invalid-process-observations")
        if sys.platform.startswith("linux"):
            guard = process.get("_pidfd")
            if not isinstance(guard, _Pidfd) or guard.pid != pid:
                raise ObservationError("missing-retained-process-identity")


def snapshot(root_pid: int, runtime_root: Path) -> list[dict]:
    """Return private owned identities/roles; fail rather than truncate evidence.

    All raw command lines are temporary and only read after ancestry admission.
    Caller must take repeated snapshots before exit to track ephemeral helpers;
    this does not discover already-reparented children from a vanished ancestor.
    Linux records retain pidfds until dropped; preserve records without JSON
    round-tripping. A dead Linux pidfd excludes a zombie from live survivors;
    reaping is separately the supervisor's responsibility.
    """
    try:
        if type(root_pid) is not int or not 0 < root_pid <= 0x7fffffff:
            raise ObservationError("invalid-root-pid")
        end = time.monotonic() + SCAN_SECONDS
        root = runtime_root.resolve(strict=True)
        if not root.is_dir():
            raise ObservationError("invalid-runtime-root")
        anchor_guard = _Pidfd(root_pid) if sys.platform.startswith("linux") else None
        anchor = _basic(root_pid)
        if anchor_guard is not None:
            anchor["_pidfd"] = anchor_guard
        native = _backend()
        if sys.platform.startswith("linux"):
            topology = {}
            for pid in _linux_pids():
                _deadline(end)
                try:
                    topology[pid] = _linux_basic(pid)["parent_pid"]
                except (FileNotFoundError, ProcessLookupError):
                    continue
        elif sys.platform == "win32":
            topology = native.topology()
        else:
            topology = native.topology(end)
        selected = {root_pid}
        for _ in range(MAX_OWNED):
            _deadline(end)
            additions = {pid for pid, parent in topology.items() if parent in selected and pid not in selected}
            if not additions:
                break
            selected.update(additions)
            if len(selected) > MAX_OWNED:
                raise ObservationError("owned-process-limit")
        records = []
        for pid in sorted(selected):
            _deadline(end)
            guard = _Pidfd(pid) if sys.platform.startswith("linux") else None
            before = _basic(pid)
            if guard is not None:
                before["_pidfd"] = guard
            if pid == root_pid and not _same(before, anchor):
                raise ObservationError("root-identity-changed")
            if before["state"] in {"Z", "zombie", "exited"}:
                raise ObservationError("owned-process-exited-during-observation")
            if sys.platform.startswith("linux"):
                image, args = _linux_details(pid)
                before["_restrictions"] = _linux_restrictions(pid)
            elif sys.platform == "win32":
                image, args, parent = native.details(pid)
                before["parent_pid"] = parent
            else:
                image, args = native.details(pid)
            before["executable"] = _owned_path(image, root)
            before["role"] = _role(args)
            del args
            before["_platform"] = sys.platform
            before["_root_pid"] = root_pid
            if not _same(before, _basic(pid)):
                raise ObservationError("process-identity-changed")
            records.append(before)
        by_pid = {record["pid"]: record for record in records}
        for record in records:
            _deadline(end)
            current = _basic(record["pid"])
            if not _same(record, current):
                raise ObservationError("process-identity-changed")
            if record["pid"] == root_pid:
                continue
            parent = by_pid.get(record["parent_pid"])
            if not parent or topology.get(record["pid"]) != record["parent_pid"]:
                raise ObservationError("process-ancestry-changed")
            if sys.platform.startswith("linux") and current["parent_pid"] != record["parent_pid"]:
                raise ObservationError("process-ancestry-changed")
            if sys.platform == "darwin":
                if current["parent_pid"] != parent["pid"] or current["_parent_unique"] != parent["_unique"]:
                    raise ObservationError("process-ancestry-changed")
            elif int(record["start_identity"]) < int(parent["start_identity"]):
                raise ObservationError("process-ancestry-changed")
        if sys.platform.startswith("linux"):
            # Observer baseline and browser baseline are separate; no inherited
            # container seccomp or namespaces are credited to the renderer.
            baseline_before = _linux_basic(os.getpid())
            baseline = _linux_restrictions(os.getpid())
            if not _same(baseline_before, _linux_basic(os.getpid())):
                raise ObservationError()
            by_pid[root_pid]["_observer_baseline"] = baseline
        return records
    except ObservationError:
        raise
    except (OSError, ValueError, KeyError, IndexError, AttributeError, UnicodeError, struct.error) as error:
        raise ObservationError(operation=getattr(error, "operation", None),
                               native_error=getattr(error, "native_error", None),
                               native_result=getattr(error, "native_result", None)) from None


def sandbox_evidence(processes: list[dict], private_job: Path) -> dict:
    """Return observations with exact scope; absence/error is never a green gate.

    private_job is a private DIRECTORY, not a Windows JobObject handle. Thus
    IsProcessInJob(NULL) measures membership in *some* job, not the supervisor's
    specific job. Atomic admission/kill-on-close is separately supervisor-owned.
    No filesystem access probe or broad confinement claim is inferred here.
    """
    result = {"verified": False, "platform": sys.platform, "renderer_count": 0,
              "scope": "owned-renderer-point-in-time", "observations": []}
    try:
        _validate_records(processes)
        end = time.monotonic() + SCAN_SECONDS
        if not private_job.is_dir():
            raise ObservationError("invalid-private-job")
        renderers = [process for process in processes if process.get("role") == "renderer"]
        result["renderer_count"] = len(renderers)
        if not renderers:
            result["reason"] = "no-observed-renderer"
            return result
        native = _backend()
        root_pid = processes[0].get("_root_pid")
        browser = next(process for process in processes if process["pid"] == root_pid)
        if not _same(browser, _basic(root_pid)):
            raise ObservationError("process-identity-changed")
        passes = []
        if sys.platform.startswith("linux"):
            host = browser["_restrictions"]
            result["browser_baseline"] = host
            result["observer_baseline"] = browser["_observer_baseline"]
            result["scope"] = "renderer-seccomp-capabilities-namespace-deltas-vs-browser-and-observer"
        elif sys.platform == "win32":
            result["job_scope"] = "any-job-not-specific-supervisor-job"
            result["private_job_access"] = "not-measured"
            result["scope"] = "renderer-primary-token-integrity-restricting-sids-appcontainer-privileges-any-job"
        else:
            C.set_errno(0)
            baseline = native.check(root_pid, None, 0)
            if baseline not in (0, 1):
                raise ObservationError("sandbox-introspection-unavailable")
            result["browser_sandboxed"] = baseline == 1
            result["scope"] = "seatbelt-sandbox-presence-and-owned-helper-ancestry-not-policy-enumeration"
        for process in renderers:
            _deadline(end)
            before = _basic(process["pid"])
            if not _same(before, process):
                raise ObservationError("process-identity-changed")
            if sys.platform.startswith("linux"):
                measured = _linux_restrictions(process["pid"])
                status, base = measured["status"], host["status"]
                filters = status.get("Seccomp_filters", [None])[0]
                base_filters = base.get("Seccomp_filters", [None])[0]
                added = status["Seccomp"][0] == 2 and (
                    base["Seccomp"][0] != 2 or
                    (filters is not None and base_filters is not None and filters > base_filters))
                changed = [name for name in measured["namespaces"]
                           if measured["namespaces"][name] != host["namespaces"][name]]
                measured["added_seccomp_vs_browser"] = added
                measured["changed_namespaces_vs_browser"] = changed
                passes.append(added and status["CapEff"] == [0] and
                              (status["NoNewPrivs"] == [1] or "user" in changed or "pid" in changed))
            elif sys.platform == "win32":
                handle = native.handle(process["pid"], 0x1000 | 0x100000)
                try:
                    if not _same(process, native.basic_handle(process["pid"], handle)):
                        raise ObservationError("process-identity-changed")
                    measured = native.token(handle)
                    passes.append(measured["integrity_rid"] <= 4096 and
                                  (measured["restricted_sid_count"] > 0 or measured["appcontainer"]) and
                                  measured["enabled_privilege_count"] == 0 and measured["in_any_job"])
                finally:
                    native.close(handle)
            else:
                C.set_errno(0)
                sandboxed = native.check(process["pid"], None, 0)
                if sandboxed not in (0, 1):
                    raise ObservationError("sandbox-introspection-unavailable")
                measured = {"seatbelt_sandboxed": sandboxed == 1,
                            "helper_ancestry_observed": True,
                            "policy_rules": "not-enumerated"}
                passes.append(sandboxed == 1 and baseline == 0)
            if not _same(process, _basic(process["pid"])):
                raise ObservationError("process-identity-changed")
            result["observations"].append(measured)
        if not _same(browser, _basic(root_pid)):
            raise ObservationError("process-identity-changed")
        result["verified"] = all(passes)
        if not result["verified"]:
            result["reason"] = "required-renderer-restrictions-not-observed"
    except (ObservationError, OSError, ValueError, KeyError, StopIteration, AttributeError, struct.error):
        result["verified"] = False
        result["reason"] = "sandbox-introspection-unavailable"
    return result


def survivors(processes: list[dict]) -> list[int]:
    """Recheck exact recorded identities; introspection errors are not death.

    This establishes only survival of recorded identities, not that no unobserved
    descendants were created between snapshots. Reused PIDs do not count.
    """
    try:
        _validate_records(processes)
        result = []
        end = time.monotonic() + SCAN_SECONDS
        for process in processes:
            _deadline(end)
            if sys.platform.startswith("linux"):
                if process["_pidfd"].live():
                    result.append(process["pid"])
                continue
            try:
                current = _basic(process["pid"])
            except (FileNotFoundError, ProcessLookupError):
                continue
            if _same(process, current) and current["state"] != "exited":
                result.append(process["pid"])
        return result
    except ObservationError:
        raise
    except (OSError, ValueError, KeyError, AttributeError, struct.error):
        raise ObservationError() from None


def terminate_process(process: dict) -> bool:
    """Request termination of one exact observed process, never a PID-only kill.

    True means the native request succeeded, not that exit/cleanup was observed.
    Linux requires pidfds, Windows uses a creation-checked process HANDLE, macOS
    requires the PID-version-guarded proc_signal_with_audittoken export.
    """
    try:
        _validate_records([process])
        pid = process["pid"]
        native = _backend()
        if sys.platform.startswith("linux"):
            if not _same(process, _linux_basic(pid)):
                return False
            signal.pidfd_send_signal(process["_pidfd"].fd, signal.SIGKILL, None, 0)
            return True
        if sys.platform == "win32":
            handle = native.handle(pid, 1 | 0x1000 | 0x100000)
            try:
                current = native.basic_handle(pid, handle)
                return _same(process, current) and current["state"] == "live" and bool(native.kill(handle, 137))
            finally:
                native.close(handle)
        if not _same(process, native.basic(pid)):
            return False
        return native.terminate(process)
    except (ObservationError, OSError, ValueError, KeyError, AttributeError, StopIteration, struct.error):
        return False


def terminate_renderers(processes: list[dict]) -> bool:
    """Request termination of every observed renderer using exact native identities.

    Chromium spare renderers share the renderer role: PID ordering does not
    identify the fixture's renderer. This bounded sampled set is not an
    exhaustive renderer census. True requires every request to succeed; the
    caller must separately observe the fixture's renderer-exit and cleanup.
    """
    try:
        _validate_records(processes)
        end = time.monotonic() + SCAN_SECONDS
        requested = 0
        succeeded = True
        for process in processes:
            if process.get("role") == "renderer":
                _deadline(end)
                # Do not short-circuit: a failed retained identity must not
                # leave later observed renderers untouched.
                sent = terminate_process(process)
                succeeded = sent and succeeded
                requested += 1
        return requested > 0 and succeeded
    except (ObservationError, AttributeError, TypeError):
        return False
