"""Fixed-mount, credential-free CLI supervisor. Repository content cannot choose mounts."""
import codecs
import hashlib
import json
import os
import re
import resource
import selectors
import signal
import subprocess
import sys
import time


def emit(value):
    print(json.dumps(value, ensure_ascii=False), flush=True)


def binary_matches(executable, expected):
    try:
        with open(executable, "rb") as source:
            digest = hashlib.sha256()
            for chunk in iter(lambda: source.read(65536), b""):
                digest.update(chunk)
            return digest.hexdigest() == expected
    except OSError:
        return False


def sandbox_prefix(executable):
    args = [executable, "--unshare-all", "--die-with-parent", "--new-session", "--clearenv",
            "--cap-drop", "ALL", "--ro-bind", "/usr/bin", "/usr/bin",
            "--ro-bind", "/usr/lib", "/usr/lib", "--symlink", "usr/bin", "/bin",
            "--symlink", "usr/lib", "/lib", "--symlink", "usr/lib64", "/lib64",
            "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp", "--dir", "/etc",
            "--dir", "/home/agent", "--setenv", "HOME", "/home/agent",
            "--dir", "/runtime/bin", "--symlink", "/usr/bin/node", "/runtime/bin/node",
            "--setenv", "PATH", "/runtime/bin", "--setenv", "LANG", "C.UTF-8"]
    for source, target in [("/usr/lib64", "/usr/lib64"), ("/usr/share/nodejs", "/usr/share/nodejs"), ("/etc/ld.so.cache", "/etc/ld.so.cache")]:
        if os.path.exists(source):
            args.extend(["--ro-bind", source, target])
    return args


def probe(executable, expected):
    if not binary_matches(executable, expected):
        return "SANDBOX_CHANGED"
    if os.geteuid() != 0:
        return "SANDBOX_ISOLATION_UNAVAILABLE"
    if not os.access("/usr/bin/node", os.X_OK):
        return "SANDBOX_RUNTIME_UNAVAILABLE"
    args = sandbox_prefix(executable) + ["--uid", "1000", "--gid", "1000", "--chdir", "/", "--", "/usr/bin/node", "--version"]
    try:
        result = subprocess.run(args, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                env={"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8"}, timeout=5, check=False)
    except (OSError, subprocess.TimeoutExpired):
        return "SANDBOX_ISOLATION_UNAVAILABLE"
    if result.returncode != 0:
        return "SANDBOX_ISOLATION_UNAVAILABLE"
    version = re.fullmatch(rb"v(\d+)\.\d+\.\d+(?:[-+][A-Za-z0-9.-]+)?", result.stdout.strip())
    if not version or int(version[1]) < 20:
        return "SANDBOX_RUNTIME_UNAVAILABLE"
    return None


def main():
    if len(sys.argv) == 4 and sys.argv[1] == "--probe-v1":
        problem = probe(sys.argv[2], sys.argv[3])
        if problem:
            emit({"event": "unavailable", "protocol": 2, "code": problem})
        else:
            emit({"event": "ready", "protocol": 2})
        return
    handshake = sys.argv[1:] == ["--handshake-v1"]
    if handshake:
        if os.geteuid() != 0 or not os.access("/usr/bin/node", os.X_OK):
            return
        emit({"event": "ready", "protocol": 1})
    line = sys.stdin.readline()
    if handshake and not line:
        return
    r = json.loads(line)
    timeout = r["timeout_ms"] / 1000
    if not 0.1 <= timeout <= 120 or not isinstance(r["args"], list) or len(r["args"]) > 16:
        raise ValueError("limits")
    if not binary_matches(r["bwrap"], r["bwrap_sha256"]):
        raise ValueError("sandbox_changed")
    # Only package and this run's work directory enter the namespace. No /mnt, /root or user home.
    args = sandbox_prefix(r["bwrap"])
    args.extend(["--ro-bind", r["package_root"], "/package", "--bind", r["work_root"], "/workspace"])
    args.extend(["--uid", "1000", "--gid", "1000", "--chdir", "/workspace", "--",
                 "/usr/bin/node", "--max-old-space-size=256", r["entry"], *r["args"]])

    def limits():
        resource.setrlimit(resource.RLIMIT_CPU, (int(timeout) + 1, int(timeout) + 2))
        resource.setrlimit(resource.RLIMIT_FSIZE, (16 * 1024 * 1024, 16 * 1024 * 1024))
        resource.setrlimit(resource.RLIMIT_NOFILE, (128, 128))

    child = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                             env={"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8"}, start_new_session=True,
                             preexec_fn=limits)
    selector = selectors.DefaultSelector()
    selector.register(sys.stdin.fileno(), selectors.EVENT_READ, "parent")
    selector.register(child.stdout, selectors.EVENT_READ, "stdout")
    selector.register(child.stderr, selectors.EVENT_READ, "stderr")
    payload, position = r["stdin"].encode("utf-8"), 0
    if payload:
        os.set_blocking(child.stdin.fileno(), False)
        selector.register(child.stdin, selectors.EVENT_WRITE, "input")
    else:
        child.stdin.close()
    decoders = {k: codecs.getincrementaldecoder("utf-8")(errors="strict") for k in ("stdout", "stderr")}
    deadline, reason, size = time.monotonic() + timeout, None, 0
    try:
        while child.poll() is None or any(k.data != "parent" for k in selector.get_map().values()):
            if reason is None and time.monotonic() >= deadline:
                reason = "timed_out"
            if reason and child.poll() is None:
                os.killpg(child.pid, signal.SIGKILL)
            for key, _ in selector.select(0.05):
                if key.data == "input":
                    try:
                        position += os.write(key.fd, payload[position:position + 4096])
                    except BlockingIOError:
                        continue
                    except BrokenPipeError:
                        position = len(payload)
                    if position >= len(payload):
                        selector.unregister(key.fileobj)
                        child.stdin.close()
                    continue
                chunk = os.read(key.fd, 4096)
                if key.data == "parent":
                    reason = reason or "canceled"
                    selector.unregister(key.fileobj)
                elif chunk:
                    size += len(chunk)
                    if size > 1024 * 1024:
                        reason = "output_limit"
                    elif (value := decoders[key.data].decode(chunk)):
                        emit({"event": key.data, "text": value})
                else:
                    tail = decoders[key.data].decode(b"", final=True)
                    if tail:
                        emit({"event": key.data, "text": tail})
                    selector.unregister(key.fileobj)
        code = child.wait()
        emit({"event": "complete", "status": reason or ("succeeded" if code == 0 else "failed"),
              "exit_code": code, "sandbox": "wsl-bubblewrap", "network": "isolated"})
    finally:
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGKILL)
            child.wait()
        selector.close()


if __name__ == "__main__":
    try:
        main()
    except BrokenPipeError:
        pass
    except Exception:
        emit({"event": "complete", "status": "unknown", "message": "隔离执行器未能完整确认结果；未回退宿主执行"})
