"""Pinned SDK admission only; no downloaded code is executed here.

The caller supplies an empty, private directory outside a checkout and owns its
cleanup, including partial extraction after failure. Callers publish only fixed
ProvisionError codes, never exception tracebacks or underlying diagnostics.
"""

import bz2
import ctypes
import hashlib
import json
import os
from pathlib import Path
import platform
import ssl
import stat
import struct
import tarfile
import tempfile
import urllib.parse
import time
import urllib.request


MAX_DOWNLOAD = 512 * 1024 * 1024
MAX_UNPACKED = 3 * 1024 * 1024 * 1024
MAX_MEMBERS = 10_000
MAX_PATH = 1024
CHUNK = 64 * 1024
DOWNLOAD_SECONDS = 600
TARGETS = {
    "linux64": ("Linux", "x86_64"),
    "windows64": ("Windows", "x86_64"),
    "macosx64": ("Darwin", "x86_64"),
    "macosarm64": ("Darwin", "arm64"),
}
FRAMEWORK = ("Release", "Chromium Embedded Framework.framework")
FRAMEWORK_LINKS = {
    ("Versions", "Current"): "A",
    ("Resources",): "Versions/Current/Resources",
    ("Libraries",): "Versions/Current/Libraries",
    ("Chromium Embedded Framework",): "Versions/Current/Chromium Embedded Framework",
}


class ProvisionError(RuntimeError):
    """A closed, publication-safe status; underlying errors are suppressed."""


def sanitized_env() -> dict[str, str]:
    """Compiler/OS allowlist, not a blacklist of known secret variable names.

    HOME, profile, temporary and Cargo directories are deliberately absent: the
    orchestration caller sets private directories for each derived execution.
    No compiler flags, Python startup hooks, dynamic loader overrides, proxy,
    GitHub command files, credentials, signing or package-manager state survive.
    """
    names = {"PATH"}
    if os.name == "nt":
        names.update({
            "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT",
            "INCLUDE", "LIB", "LIBPATH", "VSINSTALLDIR", "VCINSTALLDIR",
            "VCTOOLSINSTALLDIR", "VCTOOLSVERSION", "WINDOWSSDKDIR",
            "WINDOWSSDKVERSION", "WINDOWSSDKBINPATH", "WINDOWSSDKVERBINPATH",
            "UNIVERSALCRTSDKDIR", "UCRTVERSION", "VSCMD_ARG_TGT_ARCH",
            "VSCMD_ARG_HOST_ARCH", "VCIDEINSTALLDIR", "VSCMD_VER",
        })
    elif platform.system() == "Darwin":
        names.update({"DEVELOPER_DIR", "SDKROOT"})
    result = {key: value for key, value in os.environ.items() if key.upper() in names}
    result.update({"LANG": "C", "LC_ALL": "C"})
    return result


def _native_target(target: str) -> None:
    if target not in TARGETS or struct.calcsize("P") != 8:
        raise ProvisionError("unsupported_target")
    system, machine = TARGETS[target]
    actual = platform.machine().lower()
    actual = {"amd64": "x86_64", "aarch64": "arm64"}.get(actual, actual)
    if (platform.system(), actual) != (system, machine):
        raise ProvisionError("architecture_mismatch")
    if system == "Windows":
        # Native-machine admission also excludes ARM64 x64 emulation and WOW64.
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.GetCurrentProcess.restype = ctypes.c_void_p
        kernel.IsWow64Process2.argtypes = [
            ctypes.c_void_p, ctypes.POINTER(ctypes.c_ushort), ctypes.POINTER(ctypes.c_ushort)
        ]
        process_machine, native_machine = ctypes.c_ushort(), ctypes.c_ushort()
        if not kernel.IsWow64Process2(
            kernel.GetCurrentProcess(), ctypes.byref(process_machine), ctypes.byref(native_machine)
        ) or process_machine.value != 0 or native_machine.value != 0x8664:
            raise ProvisionError("architecture_mismatch")
    elif system == "Darwin":
        libc = ctypes.CDLL(None, use_errno=True)
        libc.sysctlbyname.argtypes = [
            ctypes.c_char_p, ctypes.c_void_p, ctypes.POINTER(ctypes.c_size_t),
            ctypes.c_void_p, ctypes.c_size_t,
        ]
        translated, size = ctypes.c_int(), ctypes.c_size_t(ctypes.sizeof(ctypes.c_int))
        result = libc.sysctlbyname(
            b"sysctl.proc_translated", ctypes.byref(translated), ctypes.byref(size), None, 0
        )
        # ENOENT is the documented native Intel case; unknown introspection fails.
        if (result == 0 and translated.value != 0) or (result != 0 and ctypes.get_errno() != 2):
            raise ProvisionError("architecture_mismatch")


def _no_link(path: Path) -> os.stat_result:
    info = path.lstat()
    if stat.S_ISLNK(info.st_mode) or getattr(info, "st_file_attributes", 0) & 0x400:
        raise ProvisionError("unsafe_work_directory")
    return info


def _private_directory(work_dir: Path) -> Path:
    if not work_dir.is_absolute() or ".." in work_dir.parts:
        raise ProvisionError("unsafe_work_directory")
    for ancestor in (work_dir, *work_dir.parents):
        if not stat.S_ISDIR(_no_link(ancestor).st_mode):
            raise ProvisionError("unsafe_work_directory")
        if (ancestor / ".git").exists():
            raise ProvisionError("unsafe_work_directory")
    info = _no_link(work_dir)
    if os.name != "nt" and (info.st_uid != os.getuid() or info.st_mode & 0o077):
        raise ProvisionError("unsafe_work_directory")
    if any(work_dir.iterdir()):
        raise ProvisionError("destination_exists")
    return work_dir


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def _download(pin: dict, admitted) -> None:
    # Fixed HTTPS endpoints only. No proxies, redirects, cookies or auth handlers.
    url = pin["url"]
    expected = "https://cef-builds.spotifycdn.com/"
    if not url.startswith(expected) or url != expected + urllib.parse.quote(
        pin["root"] + ".tar.bz2", safe=""
    ):
        raise ProvisionError("invalid_pins")
    opener = urllib.request.build_opener(
        urllib.request.ProxyHandler({}), _NoRedirect(),
        urllib.request.HTTPSHandler(context=ssl.create_default_context()),
    )
    digest = hashlib.sha256()
    count = 0
    deadline = time.monotonic() + DOWNLOAD_SECONDS
    request = urllib.request.Request(url, headers={"Accept-Encoding": "identity"})
    try:
        with opener.open(request, timeout=30) as response:
            if response.status != 200 or response.geturl() != url:
                raise ProvisionError("download_failed")
            length = response.headers.get("Content-Length")
            if length is not None and (not length.isdecimal() or int(length) > MAX_DOWNLOAD):
                raise ProvisionError("download_limit")
            if response.headers.get("Content-Encoding", "identity") != "identity":
                raise ProvisionError("download_failed")
            while True:
                if time.monotonic() > deadline:
                    raise ProvisionError("download_limit")
                block = response.read1(CHUNK)
                if not block:
                    break
                count += len(block)
                if count > MAX_DOWNLOAD:
                    raise ProvisionError("download_limit")
                admitted.write(block)
                digest.update(block)
            if count == 0 or (length is not None and count != int(length)):
                raise ProvisionError("download_failed")
    except ProvisionError:
        raise
    except Exception:
        raise ProvisionError("download_failed") from None
    if digest.hexdigest() != pin["sha256"]:
        raise ProvisionError("archive_digest_mismatch")
    admitted.flush()
    admitted.seek(0)


class _BoundedTarInfo(tarfile.TarInfo):
    @classmethod
    def frombuf(cls, buf, encoding, errors):
        member = super().frombuf(buf, encoding, errors)
        # Bound extension payloads before tarfile allocates them. The pinned GNU
        # archives need long names, not PAX, sparse or privilege extensions.
        if member.type in (tarfile.GNUTYPE_LONGNAME, tarfile.GNUTYPE_LONGLINK):
            if member.size < 0 or member.size > MAX_PATH + 1:
                raise ProvisionError("archive_limit")
        elif member.type not in (tarfile.REGTYPE, tarfile.AREGTYPE, tarfile.DIRTYPE, tarfile.SYMTYPE):
            raise ProvisionError("archive_type_rejected")
        if member.size < 0 or member.size > MAX_UNPACKED:
            raise ProvisionError("archive_limit")
        if member.mode & ~0o777:
            raise ProvisionError("archive_privilege_rejected")
        return member

    def _proc_member(self, archive):
        # GNU long-name headers also count, before recursive parser admission.
        count = getattr(archive, "_admission_headers", 0) + 1
        archive._admission_headers = count
        if count > MAX_MEMBERS:
            raise ProvisionError("archive_limit")
        return super()._proc_member(archive)


class _BoundedStream:
    def __init__(self, source):
        self.source = source
        self.count = 0

    def read(self, size):
        block = self.source.read(size)
        self.count += len(block)
        # Header/long-name/padding overhead is bounded independently of files.
        if self.count > MAX_UNPACKED + MAX_MEMBERS * 2048:
            raise ProvisionError("archive_limit")
        return block


def _parts(name: str) -> tuple[str, ...]:
    if not name or len(name.encode("utf-8")) > MAX_PATH or not name.isascii():
        raise ProvisionError("archive_path_rejected")
    parts = tuple(name.split("/"))
    if len(parts) > 32:
        raise ProvisionError("archive_path_rejected")
    for part in parts:
        if not part or part in (".", "..") or len(part) > 255 or part.endswith((".", " ")):
            raise ProvisionError("archive_path_rejected")
        if any(ord(char) < 32 or ord(char) == 127 or char in '\\:<>"|?*' for char in part):
            raise ProvisionError("archive_path_rejected")
        stem = part.split(".", 1)[0].upper()
        if stem in {"CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"} or (
            len(stem) == 4 and stem[:3] in ("COM", "LPT") and stem[3] in "123456789"
        ):
            raise ProvisionError("archive_path_rejected")
    return parts


def _framework_link(parts: tuple, member, root: str, target: str) -> tuple:
    prefix = (root, *FRAMEWORK)
    suffix = parts[len(prefix):]
    if not target.startswith("macos") or parts[:len(prefix)] != prefix:
        raise ProvisionError("archive_link_rejected")
    expected = FRAMEWORK_LINKS.get(suffix)
    if expected is None or member.linkname != expected:
        raise ProvisionError("archive_link_rejected")
    # Only these exact relative, forward, framework-local versioned links exist.
    return parts[:-1] + _parts(expected)


def _extract(admitted, pin: dict, target: str, work_dir: Path) -> Path:
    entries = {}
    aliases = set()
    links = {}
    count = total = 0
    root = pin["root"]
    _parts(root)
    with bz2.BZ2File(admitted, "rb") as decompressed:
        with tarfile.open(
            fileobj=_BoundedStream(decompressed), mode="r|", tarinfo=_BoundedTarInfo
        ) as archive:
            for member in archive:
                count += 1
                if count > MAX_MEMBERS:
                    raise ProvisionError("archive_limit")
                name = member.name
                if member.isdir() and name.endswith("/"):
                    name = name[:-1]
                parts = _parts(name)
                if parts[0] != root or (count == 1 and (parts != (root,) or not member.isdir())):
                    raise ProvisionError("archive_root_rejected")
                alias = tuple(part.casefold() for part in parts)
                if alias in aliases:
                    raise ProvisionError("archive_collision")
                if len(parts) > 1 and entries.get(parts[:-1]) != "directory":
                    raise ProvisionError("archive_parent_rejected")
                if member.mode & ~0o777 or member.pax_headers or member.sparse is not None:
                    raise ProvisionError("archive_privilege_rejected")
                if not member.isfile() and member.size != 0:
                    raise ProvisionError("archive_type_rejected")
                destination = work_dir.joinpath(*parts)
                # Parent-first archive admission plus link-last creation means
                # no writes can traverse an archive link. Recheck real parents
                # as well; the caller owns this private directory exclusively.
                for parent in (destination.parent, *destination.parent.parents):
                    _no_link(parent)
                    if parent == work_dir:
                        break
                if os.path.lexists(destination):
                    raise ProvisionError("destination_exists")
                if member.isdir():
                    destination.mkdir(mode=0o700)
                    entries[parts] = "directory"
                elif member.isfile():
                    if total + member.size > MAX_UNPACKED:
                        raise ProvisionError("archive_limit")
                    source = archive.extractfile(member)
                    if source is None:
                        raise ProvisionError("archive_type_rejected")
                    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_BINARY", 0)
                    flags |= getattr(os, "O_NOFOLLOW", 0)
                    with source, os.fdopen(os.open(destination, flags, 0o600), "wb") as output:
                        written = 0
                        while True:
                            block = source.read(CHUNK)
                            if not block:
                                break
                            written += len(block)
                            total += len(block)
                            if written > member.size or total > MAX_UNPACKED:
                                raise ProvisionError("archive_limit")
                            output.write(block)
                        if written != member.size:
                            raise ProvisionError("archive_truncated")
                    destination.chmod(0o700 if member.mode & 0o111 else 0o600)
                    entries[parts] = "file"
                elif member.issym():
                    links[parts] = (_framework_link(parts, member, root, target), member.linkname)
                    entries[parts] = "link"
                else:
                    raise ProvisionError("archive_type_rejected")
                aliases.add(alias)
    if count == 0:
        raise ProvisionError("archive_root_rejected")
    # Resolve the complete link graph against admitted real members before any
    # link is created. Cycles/dangling targets and non-directory traversals fail.
    resolved_links = {}
    for parts, (referent, linkname) in links.items():
        seen = {parts}
        while True:
            replaced = False
            for end in range(1, len(referent) + 1):
                prefix = referent[:end]
                kind = entries.get(prefix)
                if kind == "link":
                    if prefix in seen:
                        raise ProvisionError("archive_link_rejected")
                    seen.add(prefix)
                    referent = links[prefix][0] + referent[end:]
                    replaced = True
                    break
                if kind is None or (end < len(referent) and kind != "directory"):
                    raise ProvisionError("archive_link_rejected")
            if not replaced:
                break
        if referent[:3] != (root, *FRAMEWORK):
            raise ProvisionError("archive_link_rejected")
        resolved_links[parts] = entries[referent] == "directory"
    for parts, (_, linkname) in links.items():
        destination = work_dir.joinpath(*parts)
        if os.path.lexists(destination):
            raise ProvisionError("destination_exists")
        os.symlink(linkname, destination, target_is_directory=resolved_links[parts])
    return work_dir / root


def provision(target: str, work_dir: Path) -> Path:
    """Return the admitted SDK root or a fixed error; never run SDK commands."""
    try:
        _native_target(target)
        work_dir = _private_directory(Path(work_dir))
        pins_path = Path(__file__).with_name("pins.json")
        if pins_path.stat().st_size > 16 * 1024:
            raise ProvisionError("invalid_pins")
        pins = json.loads(pins_path.read_text(encoding="utf-8"))
        pin = pins["targets"][target]
        if pin["root"] != f'cef_binary_{pins["version"]}_{target}_minimal':
            raise ProvisionError("invalid_pins")
        if len(pin["sha256"]) != 64 or any(char not in "0123456789abcdef" for char in pin["sha256"]):
            raise ProvisionError("invalid_pins")
        # The anonymous/exclusive private file remains open through hashing and
        # extraction. No admitted path is re-opened after digest verification.
        with tempfile.TemporaryFile(mode="w+b", dir=work_dir) as admitted:
            _download(pin, admitted)
            return _extract(admitted, pin, target, work_dir)
    except ProvisionError:
        raise
    except Exception:
        raise ProvisionError("provision_failed") from None


def _selfcheck() -> None:
    """Small admission regression check; run only after independent review."""
    for bad in ("/escape", "a/../b", "a\\b", "C:/a", "a:b", "NUL.txt", "a/CON", "a. ", "a//b"):
        try:
            _parts(bad)
        except ProvisionError:
            pass
        else:
            raise AssertionError("unsafe path accepted")
    assert _parts("root/Release/Chromium Embedded Framework.framework") == (
        "root", "Release", "Chromium Embedded Framework.framework"
    )
    env = sanitized_env()
    assert not {"GITHUB_ENV", "GITHUB_OUTPUT", "GITHUB_TOKEN", "GH_TOKEN", "HOME", "USERPROFILE", "SSH_AUTH_SOCK", "CARGO_HOME"} & {key.upper() for key in env}
    member = tarfile.TarInfo("root/Release/Chromium Embedded Framework.framework/Versions/Current")
    member.linkname = "A"
    assert _framework_link(_parts(member.name), member, "root", "macosarm64") == (
        "root", *FRAMEWORK, "Versions", "A"
    )
    import io
    from unittest.mock import patch

    with patch.dict(os.environ, {"GITHUB_ENV": "/secret", "GH_TOKEN": "secret", "CARGO_HOME": "/secret"}):
        assert not {"GITHUB_ENV", "GH_TOKEN", "CARGO_HOME"} & set(sanitized_env())
    # Tiny real tar streams exercise extraction and rejection without network,
    # any pinned SDK input, subprocesses or downloaded code.
    for bad in (None, "collision", "privilege", "hardlink", "escape", "parent"):
        admitted = io.BytesIO()
        with tarfile.open(fileobj=admitted, mode="w:bz2", format=tarfile.GNU_FORMAT) as archive:
            root = tarfile.TarInfo("root")
            root.type, root.mode = tarfile.DIRTYPE, 0o755
            archive.addfile(root)
            file = tarfile.TarInfo("root/file")
            file.mode, file.size = 0o644, 1
            archive.addfile(file, io.BytesIO(b"x"))
            if bad is not None:
                extra = tarfile.TarInfo("root/extra")
                extra.mode = 0o644
                if bad == "collision":
                    extra.name = "root/FILE"
                elif bad == "privilege":
                    extra.mode = 0o4755
                elif bad == "hardlink":
                    extra.type, extra.linkname = tarfile.LNKTYPE, "root/file"
                elif bad == "escape":
                    extra.name = "root/../escape"
                else:
                    extra.name = "root/file/child"
                archive.addfile(extra)
        admitted.seek(0)
        with tempfile.TemporaryDirectory() as directory:
            try:
                sdk = _extract(admitted, {"root": "root"}, "linux64", Path(directory).resolve())
            except ProvisionError:
                assert bad is not None
            else:
                assert bad is None and (sdk / "file").read_bytes() == b"x"


if __name__ == "__main__":
    _selfcheck()
