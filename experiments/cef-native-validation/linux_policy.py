"""Root-only, bounded AppArmor userns exception for disposable Linux CI.

This copies only the pinned Linux CEF runtime tree. It never executes CEF,
changes sysctls, installs the setuid helper, or follows source/destination links.
The attached AppArmor profile grants `userns`; it does not confine filesystem access.
"""

import hashlib
import os
import platform
import re
import signal
import stat
import subprocess
import sys


INSTALL_BASE = "/usr/lib/eutheto-cef-native-validation"
PROFILE_DIR = "/etc/apparmor.d"
PROFILE_PREFIX = "eutheto-cef-native-validation-"
OWNER_FILE = ".cef-policy-owner"
PROFILE_CLAIM = ".cef-policy-profile"
MAX_MEMBERS = 10_000
MAX_BYTES = 3 * 1024 * 1024 * 1024
MAX_PRIVATE_MEMBERS = MAX_MEMBERS + 2
MAX_PRIVATE_BYTES = MAX_BYTES + 4096
MAX_PATH = 1024
CHUNK = 64 * 1024
INSTALL_SECONDS = 120
CLEANUP_SECONDS = 30
PARSER_SECONDS = 10
MAX_PROFILES = 4 * 1024 * 1024

RUNTIME_FILES = {
    "cef-probe",
    "libcef.so",
    "libvk_swiftshader.so",
    "libvulkan.so.1",
    "v8_context_snapshot.bin",
    "vk_swiftshader_icd.json",
    "chrome_100_percent.pak",
    "chrome_200_percent.pak",
    "resources.pak",
    "icudtl.dat",
}

DIR_FLAGS = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
FILE_FLAGS = os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC


class Refused(RuntimeError):
    """Closed failure; caller output never includes private diagnostics."""


def _require(condition):
    if not condition:
        raise Refused


def _alarm(signum, frame):
    raise Refused


def _deadline(seconds):
    signal.signal(signal.SIGALRM, _alarm)
    signal.signal(signal.SIGTERM, _alarm)
    signal.signal(signal.SIGINT, _alarm)
    signal.setitimer(signal.ITIMER_REAL, seconds)


def _stop_deadline():
    signal.setitimer(signal.ITIMER_REAL, 0)


def _write_all(fd, data):
    view = memoryview(data)
    while view:
        written = os.write(fd, view)
        _require(written > 0)
        view = view[written:]


def _read_bounded(fd, limit):
    data = bytearray()
    while len(data) <= limit:
        block = os.read(fd, min(CHUNK, limit + 1 - len(data)))
        if not block:
            return bytes(data)
        data.extend(block)
    raise Refused


def _iter_names(fd):
    with os.scandir(fd) as entries:
        for entry in entries:
            yield entry.name


def _valid_component(name):
    try:
        encoded = name.encode("ascii")
    except UnicodeEncodeError:
        raise Refused from None
    _require(0 < len(encoded) <= 255 and name not in (".", ".."))
    _require(not any(ord(char) < 32 or ord(char) == 127 or char in "/\\" for char in name))
    return encoded


def _absolute_parts(path):
    _require(path.startswith("/") and not path.startswith("//") and not path.endswith("/"))
    parts = path.split("/")[1:]
    _require(parts and all(parts) and all(part not in (".", "..") for part in parts))
    _require(len(path.encode("utf-8", "strict")) <= MAX_PATH)
    return parts


def _check_system_dir(fd):
    info = os.fstat(fd)
    _require(stat.S_ISDIR(info.st_mode) and info.st_uid == 0 and not (info.st_mode & 0o022))
    _require(not (info.st_mode & (stat.S_ISUID | stat.S_ISGID | stat.S_ISVTX)))


def _open_system_dir(path, create_last=False):
    parts = _absolute_parts(path)
    fd = os.open("/", DIR_FLAGS)
    try:
        _check_system_dir(fd)
        for index, part in enumerate(parts):
            _valid_component(part)
            try:
                child = os.open(part, DIR_FLAGS, dir_fd=fd)
            except FileNotFoundError:
                _require(create_last and index == len(parts) - 1)
                os.mkdir(part, 0o755, dir_fd=fd)
                child = os.open(part, DIR_FLAGS, dir_fd=fd)
                os.fchmod(child, 0o755)
            _check_system_dir(child)
            os.close(fd)
            fd = child
        return fd
    except BaseException:
        os.close(fd)
        raise


def _caller_uid():
    _require(os.geteuid() == 0)
    value = os.environ.get("SUDO_UID", "")
    _require(re.fullmatch(r"[1-9][0-9]{0,9}", value) is not None)
    uid = int(value)
    _require(uid < 2**32 - 1 and os.getuid() in (0, uid))
    return uid


def _open_source_tree(path):
    parts = _absolute_parts(path)
    _require(len(parts) >= 3 and parts[-2:] == ["build", "Release"])
    work_parts = parts[:-2]
    uid = _caller_uid()
    _require(work_parts)
    current = os.open("/", DIR_FLAGS)
    try:
        for part in work_parts:
            _valid_component(part)
            child = os.open(part, DIR_FLAGS, dir_fd=current)
            os.close(current)
            current = child
        work = os.fstat(current)
        _require(
            stat.S_ISDIR(work.st_mode) and work.st_uid == uid and work.st_mode & 0o700 == 0o700
            and not (work.st_mode & 0o022)
            and not (work.st_mode & (stat.S_ISUID | stat.S_ISGID | stat.S_ISVTX))
        )
        build_fd = os.open("build", DIR_FLAGS, dir_fd=current)
        build = os.fstat(build_fd)
        _require(build.st_uid == uid and not (build.st_mode & 0o022))
        _require(not (build.st_mode & (stat.S_ISUID | stat.S_ISGID | stat.S_ISVTX)))
        source_fd = os.open("Release", DIR_FLAGS, dir_fd=build_fd)
        try:
            source = os.fstat(source_fd)
            _require(source.st_uid == uid and not (source.st_mode & 0o022))
            _require(not (source.st_mode & (stat.S_ISUID | stat.S_ISGID | stat.S_ISVTX)))
            return source_fd, source.st_dev, uid
        except BaseException:
            os.close(source_fd)
            raise
    finally:
        os.close(current)
        if "build_fd" in locals():
            os.close(build_fd)


def _host_policy():
    _caller_uid()
    _require(platform.freedesktop_os_release().get("ID") == "ubuntu")
    with open("/proc/sys/kernel/apparmor_restrict_unprivileged_userns", "rb") as stream:
        _require(stream.read(32).strip() == b"1" and stream.read(1) == b"")
    with open("/sys/module/apparmor/parameters/enabled", "rb") as stream:
        _require(stream.read(32).strip() == b"Y" and stream.read(1) == b"")


def _trusted_parser():
    for candidate in ("/usr/sbin/apparmor_parser", "/usr/bin/apparmor_parser"):
        resolved = os.path.realpath(candidate)
        if not resolved.startswith("/"):
            continue
        parts = _absolute_parts(resolved)
        parent_fd = _open_system_dir("/" + "/".join(parts[:-1]))
        try:
            info = os.stat(parts[-1], dir_fd=parent_fd, follow_symlinks=False)
            if (
                stat.S_ISREG(info.st_mode) and info.st_uid == 0
                and not (info.st_mode & (0o022 | stat.S_ISUID | stat.S_ISGID | stat.S_ISVTX))
                and info.st_mode & 0o111
            ):
                return resolved
        except OSError:
            pass
        finally:
            os.close(parent_fd)
    raise Refused


def _profile_dir_fd():
    return _open_system_dir(PROFILE_DIR)


def _loaded_profiles():
    apparmor_fd = _open_system_dir("/sys/kernel/security/apparmor")
    try:
        fd = os.open("profiles", FILE_FLAGS, dir_fd=apparmor_fd)
        try:
            info = os.fstat(fd)
            _require(stat.S_ISREG(info.st_mode) and info.st_uid == 0)
            data = _read_bounded(fd, MAX_PROFILES)
        finally:
            os.close(fd)
    finally:
        os.close(apparmor_fd)
    try:
        lines = data.decode("utf-8", "strict").splitlines()
        names = set()
        for line in lines:
            name, state = line.rsplit(" (", 1)
            _require(state.endswith(")") and name)
            names.add(name)
        return names
    except (UnicodeError, ValueError):
        raise Refused from None


def _parser_command(parser, action, profile_path):
    process = subprocess.Popen(
        [parser, "-K", action, profile_path],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        cwd="/",
        env={"PATH": "/usr/sbin:/usr/bin:/sbin:/bin", "LC_ALL": "C"},
        close_fds=True,
        start_new_session=True,
    )
    try:
        return process.wait(timeout=PARSER_SECONDS)
    except BaseException:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=2)
        except BaseException:
            pass
        raise


def _identity(run, attempt, token):
    _require(re.fullmatch(r"[1-9][0-9]{0,19}", run) is not None)
    _require(re.fullmatch(r"[1-9][0-9]{0,7}", attempt) is not None)
    _require(re.fullmatch(r"[0-9a-f]{32}", token) is not None)
    return f"{run}-{attempt}"


def _profile_label(run, attempt, token):
    return f"{PROFILE_PREFIX}{run}-{attempt}-{token}"


def _runtime_path(run, attempt):
    return f"{INSTALL_BASE}/{run}-{attempt}"


def _profile_path(run, attempt):
    return f"{PROFILE_DIR}/{PROFILE_PREFIX}{run}-{attempt}"


def _profile_bytes(run, attempt, token):
    runtime = _runtime_path(run, attempt)
    label = _profile_label(run, attempt, token)
    return (
        "abi <abi/4.0>,\n"
        f"profile {label} {runtime}/cef-probe flags=(unconfined) {{\n"
        "  userns,\n"
        "}\n"
    ).encode("ascii")


def _write_owned_file(dir_fd, name, data, mode, created=None):
    mask = {signal.SIGALRM, signal.SIGTERM, signal.SIGINT}
    old_mask = signal.pthread_sigmask(signal.SIG_BLOCK, mask)
    fd = -1
    try:
        fd = os.open(
            name,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC,
            0o600,
            dir_fd=dir_fd,
        )
        if created is not None:
            created[0] = True
        try:
            signal.pthread_sigmask(signal.SIG_SETMASK, old_mask)
            old_mask = None
            _write_all(fd, data)
            os.fchown(fd, 0, 0)
            os.fchmod(fd, mode)
            os.fsync(fd)
            os.fsync(dir_fd)
        except BaseException:
            try:
                os.unlink(name, dir_fd=dir_fd)
            except OSError:
                pass
            raise
    finally:
        if fd >= 0:
            os.close(fd)
        if old_mask is not None:
            signal.pthread_sigmask(signal.SIG_SETMASK, old_mask)


def _read_claim(dir_fd, name, limit):
    try:
        fd = os.open(name, FILE_FLAGS, dir_fd=dir_fd)
    except FileNotFoundError:
        return None
    try:
        info = os.fstat(fd)
        _require(
            stat.S_ISREG(info.st_mode) and info.st_uid == 0 and info.st_gid == 0
            and stat.S_IMODE(info.st_mode) == 0o600 and info.st_nlink == 1
            and info.st_size <= limit
        )
        return _read_bounded(fd, limit)
    finally:
        os.close(fd)


def _verify_root_tree(fd, limit_members=MAX_PRIVATE_MEMBERS, limit_bytes=MAX_PRIVATE_BYTES):
    state = [0, 0]

    def walk(directory_fd, depth):
        _require(depth <= 32)
        for name in _iter_names(directory_fd):
            _valid_component(name)
            state[0] += 1
            _require(state[0] <= limit_members)
            info = os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
            _require(info.st_uid == 0 and info.st_gid == 0 and not (info.st_mode & 0o022))
            _require(not (info.st_mode & (stat.S_ISUID | stat.S_ISGID | stat.S_ISVTX)))
            if stat.S_ISDIR(info.st_mode):
                child = os.open(name, DIR_FLAGS, dir_fd=directory_fd)
                try:
                    child_info = os.fstat(child)
                    _require(child_info.st_dev == info.st_dev and child_info.st_ino == info.st_ino)
                    walk(child, depth + 1)
                finally:
                    os.close(child)
            elif stat.S_ISREG(info.st_mode):
                _require(info.st_nlink == 1)
                state[1] += info.st_size
                _require(state[1] <= limit_bytes)
            else:
                raise Refused

    walk(fd, 0)


def _remove_tree_contents(fd, preserve=()):
    state = [0, 0]

    def walk(directory_fd, depth, root=False):
        _require(depth <= 32)
        for name in _iter_names(directory_fd):
            _valid_component(name)
            state[0] += 1
            _require(state[0] <= MAX_PRIVATE_MEMBERS)
            info = os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
            _require(info.st_uid == 0 and info.st_gid == 0 and not (info.st_mode & 0o022))
            _require(not (info.st_mode & (stat.S_ISUID | stat.S_ISGID | stat.S_ISVTX)))
            if root and name in preserve:
                continue
            if stat.S_ISDIR(info.st_mode):
                child = os.open(name, DIR_FLAGS, dir_fd=directory_fd)
                try:
                    child_info = os.fstat(child)
                    _require(child_info.st_dev == info.st_dev and child_info.st_ino == info.st_ino)
                    walk(child, depth + 1)
                finally:
                    os.close(child)
                os.rmdir(name, dir_fd=directory_fd)
            elif stat.S_ISREG(info.st_mode):
                _require(info.st_nlink == 1)
                state[1] += info.st_size
                _require(state[1] <= MAX_PRIVATE_BYTES)
                os.unlink(name, dir_fd=directory_fd)
            else:
                raise Refused

    walk(fd, 0, root=True)


def _profile_file(dir_fd, name, expected):
    try:
        fd = os.open(name, FILE_FLAGS, dir_fd=dir_fd)
    except FileNotFoundError:
        return False
    try:
        info = os.fstat(fd)
        _require(
            stat.S_ISREG(info.st_mode) and info.st_uid == 0 and info.st_gid == 0
            and stat.S_IMODE(info.st_mode) == 0o644 and info.st_nlink == 1
            and info.st_size == len(expected)
        )
        _require(_read_bounded(fd, len(expected)) == expected)
        return True
    finally:
        os.close(fd)


def _cleanup(run, attempt, token, allow_unclaimed_profile=False):
    uid = _caller_uid()
    identity = _identity(run, attempt, token)
    try:
        base_fd = _open_system_dir(INSTALL_BASE)
    except FileNotFoundError:
        return
    try:
        try:
            install_fd = os.open(identity, DIR_FLAGS, dir_fd=base_fd)
        except FileNotFoundError:
            return
        try:
            install_info = os.fstat(install_fd)
            _require(
                stat.S_ISDIR(install_info.st_mode) and install_info.st_uid == 0
                and install_info.st_gid == 0 and not (install_info.st_mode & 0o022)
                and not (install_info.st_mode & (stat.S_ISUID | stat.S_ISGID | stat.S_ISVTX))
            )
            owner = _read_claim(install_fd, OWNER_FILE, 128)
            _require(owner == f"CEF-LINUX-POLICY-V1\n{uid}\n{token}\n".encode("ascii"))
            _verify_root_tree(install_fd)

            profile_claim = _read_claim(install_fd, PROFILE_CLAIM, 128)
            expected_profile = _profile_bytes(run, attempt, token)
            expected_claim = f"{token}\n{hashlib.sha256(expected_profile).hexdigest()}\n".encode("ascii")
            if profile_claim is not None:
                _require(profile_claim == expected_claim)
            claimed = profile_claim is not None or allow_unclaimed_profile
            if claimed:
                profile_fd = _profile_dir_fd()
                profile_name = _profile_path(run, attempt).rsplit("/", 1)[1]
                profile_label = _profile_label(run, attempt, token)
                try:
                    profile_exists = _profile_file(profile_fd, profile_name, expected_profile)
                    loaded = _loaded_profiles()
                    if profile_exists:
                        if profile_label in loaded:
                            parser = _trusted_parser()
                            result = _parser_command(parser, "-R", _profile_path(run, attempt))
                            loaded = _loaded_profiles()
                            _require(result == 0 or profile_label not in loaded)
                            _require(profile_label not in loaded)
                        os.unlink(profile_name, dir_fd=profile_fd)
                        os.fsync(profile_fd)
                    else:
                        _require(profile_label not in loaded)
                finally:
                    os.close(profile_fd)

            _remove_tree_contents(install_fd, preserve={OWNER_FILE, PROFILE_CLAIM})
            remaining = set()
            for name in _iter_names(install_fd):
                _require(len(remaining) < 2 and name not in remaining)
                remaining.add(name)
            _require(OWNER_FILE in remaining and remaining <= {OWNER_FILE, PROFILE_CLAIM})
            old_mask = signal.pthread_sigmask(
                signal.SIG_BLOCK, {signal.SIGALRM, signal.SIGTERM, signal.SIGINT}
            )
            try:
                if PROFILE_CLAIM in remaining:
                    os.unlink(PROFILE_CLAIM, dir_fd=install_fd)
                os.unlink(OWNER_FILE, dir_fd=install_fd)
                os.fsync(install_fd)
                current = os.stat(identity, dir_fd=base_fd, follow_symlinks=False)
                _require(
                    current.st_dev == install_info.st_dev and current.st_ino == install_info.st_ino
                    and stat.S_ISDIR(current.st_mode)
                )
                os.rmdir(identity, dir_fd=base_fd)
                os.fsync(base_fd)
            finally:
                signal.pthread_sigmask(signal.SIG_SETMASK, old_mask)
        finally:
            os.close(install_fd)
    finally:
        os.close(base_fd)


def _source_file(src_dir_fd, name, expected, source_device, uid):
    path_fd = os.open(name, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=src_dir_fd)
    try:
        before = os.fstat(path_fd)
        _require(
            stat.S_ISREG(before.st_mode) and before.st_dev == expected.st_dev
            and before.st_ino == expected.st_ino and before.st_mode == expected.st_mode
            and before.st_size == expected.st_size and before.st_uid == uid
            and before.st_dev == source_device and before.st_nlink == 1
            and not (before.st_mode & 0o022)
            and not (before.st_mode & (stat.S_ISUID | stat.S_ISGID | stat.S_ISVTX))
        )
        data_fd = os.open(f"/proc/self/fd/{path_fd}", os.O_RDONLY | os.O_CLOEXEC)
        return path_fd, data_fd, before
    except BaseException:
        os.close(path_fd)
        raise


def _allowed_runtime_path(parts, is_directory):
    if len(parts) == 1:
        return (parts[0] == "locales" and is_directory) or (parts[0] in RUNTIME_FILES and not is_directory)
    return (
        len(parts) == 2 and parts[0] == "locales" and not is_directory
        and re.fullmatch(r"[A-Za-z0-9_-]+\.pak", parts[1]) is not None
    )


def _copy_runtime(source_fd, destination_fd, source_device, uid):
    counts = {"members": 0, "bytes": 0, "root": set(), "locales": 0}

    def copy_dir(src_fd, dst_fd, parts, depth):
        _require(depth <= 32)
        for name in _iter_names(src_fd):
            _valid_component(name)
            child_parts = parts + (name,)
            _require(len("/".join(child_parts).encode("ascii")) <= MAX_PATH)
            info = os.stat(name, dir_fd=src_fd, follow_symlinks=False)
            is_directory = stat.S_ISDIR(info.st_mode)
            _require(_allowed_runtime_path(child_parts, is_directory))
            counts["members"] += 1
            _require(counts["members"] <= MAX_MEMBERS)
            if parts == ("locales",) and not is_directory:
                counts["locales"] += 1
            if not parts:
                counts["root"].add(name)

            if is_directory:
                src_child = os.open(name, DIR_FLAGS, dir_fd=src_fd)
                try:
                    source_info = os.fstat(src_child)
                    _require(
                        source_info.st_dev == info.st_dev == source_device
                        and source_info.st_ino == info.st_ino and source_info.st_uid == uid
                        and not (source_info.st_mode & 0o022)
                        and not (source_info.st_mode & (stat.S_ISUID | stat.S_ISGID | stat.S_ISVTX))
                    )
                    os.mkdir(name, 0o700, dir_fd=dst_fd)
                    dst_child = os.open(name, DIR_FLAGS, dir_fd=dst_fd)
                    try:
                        copy_dir(src_child, dst_child, child_parts, depth + 1)
                        os.fchmod(dst_child, 0o755)
                        os.fsync(dst_child)
                    finally:
                        os.close(dst_child)
                finally:
                    os.close(src_child)
            else:
                src_path_fd, src_data_fd, before = _source_file(
                    src_fd, name, info, source_device, uid
                )
                out_fd = -1
                try:
                    data_info = os.fstat(src_data_fd)
                    _require(data_info.st_dev == before.st_dev and data_info.st_ino == before.st_ino)
                    _require(before.st_size >= 0 and counts["bytes"] + before.st_size <= MAX_BYTES)
                    out_fd = os.open(
                        name,
                        os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC,
                        0o600,
                        dir_fd=dst_fd,
                    )
                    written = 0
                    while True:
                        block = os.read(src_data_fd, CHUNK)
                        if not block:
                            break
                        written += len(block)
                        counts["bytes"] += len(block)
                        _require(written <= before.st_size and counts["bytes"] <= MAX_BYTES)
                        _write_all(out_fd, block)
                    after = os.fstat(src_path_fd)
                    _require(written == before.st_size and _stable_file(before, after))
                    mode = 0o755 if before.st_mode & 0o111 else 0o644
                    os.fchown(out_fd, 0, 0)
                    os.fchmod(out_fd, mode)
                    target_info = os.fstat(out_fd)
                    _require(
                        target_info.st_uid == 0 and target_info.st_gid == 0
                        and stat.S_IMODE(target_info.st_mode) == mode and target_info.st_size == written
                    )
                    os.fsync(out_fd)
                finally:
                    if out_fd >= 0:
                        os.close(out_fd)
                    os.close(src_data_fd)
                    os.close(src_path_fd)
        os.fsync(dst_fd)

    def _stable_file(before, after):
        return (
            before.st_dev, before.st_ino, before.st_uid, before.st_gid,
            before.st_mode, before.st_nlink, before.st_size,
            before.st_mtime_ns, before.st_ctime_ns,
        ) == (
            after.st_dev, after.st_ino, after.st_uid, after.st_gid,
            after.st_mode, after.st_nlink, after.st_size,
            after.st_mtime_ns, after.st_ctime_ns,
        )

    copy_dir(source_fd, destination_fd, (), 0)
    _require(counts["root"] == RUNTIME_FILES | {"locales"} and counts["locales"] > 0)
    host = os.open("cef-probe", FILE_FLAGS, dir_fd=destination_fd)
    try:
        info = os.fstat(host)
        _require(stat.S_ISREG(info.st_mode) and info.st_uid == 0 and info.st_gid == 0 and info.st_mode & 0o111)
    finally:
        os.close(host)
    os.fchmod(destination_fd, 0o755)
    os.fsync(destination_fd)


def _install(source_path, run, attempt, token):
    identity = _identity(run, attempt, token)
    _host_policy()
    parser = _trusted_parser()
    profile_dir_fd = _profile_dir_fd()
    source_fd, source_device, uid = _open_source_tree(source_path)
    base_fd = _open_system_dir(INSTALL_BASE, create_last=True)
    destination_claimed = False
    marker_created = False
    profile_created = [False]
    destination_fd = -1
    try:
        old_mask = signal.pthread_sigmask(
            signal.SIG_BLOCK, {signal.SIGALRM, signal.SIGTERM, signal.SIGINT}
        )
        try:
            os.mkdir(identity, 0o700, dir_fd=base_fd)
            destination_claimed = True
        finally:
            signal.pthread_sigmask(signal.SIG_SETMASK, old_mask)
        destination_fd = os.open(identity, DIR_FLAGS, dir_fd=base_fd)
        dest_info = os.fstat(destination_fd)
        _require(dest_info.st_uid == 0 and dest_info.st_gid == 0 and stat.S_IMODE(dest_info.st_mode) == 0o700)
        _write_owned_file(
            destination_fd,
            OWNER_FILE,
            f"CEF-LINUX-POLICY-V1\n{uid}\n{token}\n".encode("ascii"),
            0o600,
        )
        marker_created = True
        _copy_runtime(source_fd, destination_fd, source_device, uid)

        profile_label = _profile_label(run, attempt, token)
        _require(profile_label not in _loaded_profiles())
        profile_name = _profile_path(run, attempt).rsplit("/", 1)[1]
        profile_data = _profile_bytes(run, attempt, token)
        profile_path = _profile_path(run, attempt)
        _write_owned_file(profile_dir_fd, profile_name, profile_data, 0o644, profile_created)
        _write_owned_file(
            destination_fd,
            PROFILE_CLAIM,
            f"{token}\n{hashlib.sha256(profile_data).hexdigest()}\n".encode("ascii"),
            0o600,
        )
        _require(_parser_command(parser, "-a", profile_path) == 0)
        _require(profile_label in _loaded_profiles())
    except BaseException:
        if destination_claimed:
            _stop_deadline()
            _deadline(30)
            try:
                if marker_created:
                    _cleanup(run, attempt, token, allow_unclaimed_profile=profile_created[0])
                elif destination_fd >= 0:
                    _remove_tree_contents(destination_fd)
                    os.close(destination_fd)
                    destination_fd = -1
                    os.rmdir(identity, dir_fd=base_fd)
                else:
                    info = os.stat(identity, dir_fd=base_fd, follow_symlinks=False)
                    _require(stat.S_ISDIR(info.st_mode) and info.st_uid == 0)
                    os.rmdir(identity, dir_fd=base_fd)
            except BaseException:
                pass
        raise
    finally:
        if destination_fd >= 0:
            os.close(destination_fd)
        os.close(base_fd)
        os.close(source_fd)
        os.close(profile_dir_fd)


def _cleanup_command(run, attempt, token):
    _identity(run, attempt, token)
    _cleanup(run, attempt, token)


def _run(argv):
    if len(argv) == 5 and argv[0] == "install":
        _identity(argv[2], argv[3], argv[4])
        _deadline(INSTALL_SECONDS)
        try:
            _install(argv[1], argv[2], argv[3], argv[4])
        finally:
            _stop_deadline()
        return "installed"
    if len(argv) == 4 and argv[0] == "cleanup":
        _identity(argv[1], argv[2], argv[3])
        _deadline(CLEANUP_SECONDS)
        try:
            _cleanup_command(argv[1], argv[2], argv[3])
        finally:
            _stop_deadline()
        return "cleaned"
    raise Refused


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    status = "invalid_request"
    result = 64
    try:
        status = _run(argv)
        result = 0
    except BaseException:
        if argv and argv[0] == "install":
            status, result = "install_failed", 1
        elif argv and argv[0] == "cleanup":
            status, result = "cleanup_failed", 1
        else:
            status, result = "invalid_request", 64
    try:
        sys.stdout.write(status + "\n")
        sys.stdout.flush()
    except OSError:
        pass
    return result


if __name__ == "__main__":
    raise SystemExit(main())
