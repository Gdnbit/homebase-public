"""Credential-free WSL supervisor. Input is trusted host configuration, never model-selected mounts."""
import json
import codecs
import os
import resource
import selectors
import signal
import subprocess
import sys
import tempfile
import time


def emit(value):
    print(json.dumps(value, ensure_ascii=False), flush=True)


def main():
    request = json.loads(sys.stdin.readline())
    timeout = request["timeout_ms"] / 1000
    command = request["command"]
    if not isinstance(command, str) or not 0 < len(command) <= 16000 or not 0.1 <= timeout <= 300:
        raise ValueError("invalid_request")
    with tempfile.TemporaryDirectory(prefix="homebase-mask-") as temporary:
        blocked_file = os.path.join(temporary, "blocked")
        with open(blocked_file, "wb") as stream:
            stream.write(b"protected")
        os.chmod(blocked_file, 0)
        blocked_directory = os.path.join(temporary, "directory")
        os.mkdir(blocked_directory, 0)
        args = [request["executable"], "--unshare-all", "--die-with-parent", "--new-session",
                "--clearenv", "--cap-drop", "ALL", "--ro-bind", "/usr", "/usr",
                "--symlink", "usr/bin", "/bin", "--symlink", "usr/lib", "/lib",
                "--symlink", "usr/lib64", "/lib64", "--proc", "/proc", "--dev", "/dev",
                "--tmpfs", "/tmp", "--dir", "/home/agent", "--dir", "/etc",
                "--setenv", "HOME", "/home/agent", "--setenv", "PATH", "/usr/bin:/bin",
                "--setenv", "LANG", "C.UTF-8", "--setenv", "PYTHONIOENCODING", "utf-8"]
        if os.path.isfile("/etc/ld.so.cache"):
            args.extend(["--ro-bind", "/etc/ld.so.cache", "/etc/ld.so.cache"])
        args.extend(["--bind" if request["write"] else "--ro-bind", request["workspace"], "/workspace"])
        for mask in request["masks"]:
            args.extend(["--ro-bind", blocked_directory if mask["directory"] else blocked_file,
                         "/workspace/" + mask["path"]])
        args.extend(["--uid", "1000", "--gid", "1000", "--chdir", "/workspace" + request["cwd"],
                     "--", "/bin/bash", "--noprofile", "--norc", "-c", command])

        def child_limits():
            resource.setrlimit(resource.RLIMIT_CPU, (max(1, int(timeout)), max(2, int(timeout) + 1)))
            resource.setrlimit(resource.RLIMIT_FSIZE, (32 * 1024 * 1024, 32 * 1024 * 1024))
            resource.setrlimit(resource.RLIMIT_NOFILE, (256, 256))

        child = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE, start_new_session=True,
                                 env={"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8"},
                                 preexec_fn=child_limits)
        selector = selectors.DefaultSelector()
        selector.register(sys.stdin.fileno(), selectors.EVENT_READ, "host")
        selector.register(child.stdout, selectors.EVENT_READ, "stdout")
        selector.register(child.stderr, selectors.EVENT_READ, "stderr")
        deadline = time.monotonic() + timeout
        reason = None
        decoders = {name: codecs.getincrementaldecoder("utf-8")(errors="replace") for name in ("stdout", "stderr")}
        try:
            while child.poll() is None or any(k.data != "host" for k in selector.get_map().values()):
                if reason is None and time.monotonic() >= deadline:
                    reason = "timed_out"
                if reason is not None and child.poll() is None:
                    os.killpg(child.pid, signal.SIGKILL)
                for key, _ in selector.select(0.05):
                    chunk = os.read(key.fd, 4096)
                    if key.data == "host":
                        # Closing the parent pipe (including host death) cancels the whole sandbox.
                        reason = reason or "canceled"
                        selector.unregister(key.fileobj)
                    elif chunk:
                        text = decoders[key.data].decode(chunk)
                        if text:
                            emit({"event": key.data, "text": text})
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


try:
    main()
except BrokenPipeError:
    pass
except Exception as error:
    # No raw exception paths, environment values, request or provider data.
    emit({"event": "complete", "status": "failed", "error": type(error).__name__,
          "message": "隔离执行器启动或监督失败；未退回宿主执行"})
