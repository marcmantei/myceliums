"""A copy of the JDK whose ``java`` always uses IPv4 sockets (#183).

    python3 -I make_jdk_ipv4.py ~/.cache/myc-java/jdk/jdk-25.0.5.1+2 ~/.cache/myc-java/jdk-ipv4

Under macOS's sandbox the loopback rule does not cover IPv4-mapped IPv6
sockets, which a JVM opens by default, and Gradle's process-isolated workers
receive neither an environment nor a system property. So the flag goes where
every JVM passes: in the copy ``bin/java`` is a script that runs the renamed
launcher (``bin/java.real``) with ``-Djava.net.preferIPv4Stack=true`` and its
arguments. ``java.home`` is the copy. Nothing else differs, and the original
is not touched (its files are hashed before and after). The manifest lists
every file that differs from the original. Use the copy offline only: to a
firewall it is another program.
"""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
from pathlib import Path

FLAG = "-Djava.net.preferIPv4Stack=true"


def hashes(root: Path) -> dict[str, str]:
    out = {}
    for p in sorted(root.rglob("*")):
        rel = str(p.relative_to(root))
        if p.is_symlink():
            out[rel] = "link:" + str(p.readlink())
        elif p.is_file():
            out[rel] = hashlib.sha256(p.read_bytes()).hexdigest()
    return out


def main() -> int:
    src, dst = (Path(a).expanduser().resolve() for a in sys.argv[1:3])
    if dst.exists():
        sys.exit(f"{dst} exists")
    before = hashes(src)
    subprocess.run(["cp", "-c", "-R", str(src), str(dst)], check=True)
    java = dst / "Contents" / "Home" / "bin" / "java"
    real = java.with_name("java.real")
    java.rename(real)
    java.write_text(f'#!/bin/sh\nexec "{real}" {FLAG} "$@"\n')
    java.chmod(0o755)
    after = hashes(src)
    if after != before:
        sys.exit("the original JDK changed")
    copy = hashes(dst)
    diff = {k: {"original": before.get(k), "copy": copy.get(k)} for k in sorted(set(before) | set(copy))
            if before.get(k) != copy.get(k)}
    manifest = {"original": str(src), "copy": str(dst), "flag": FLAG, "files": len(before),
                "original_tree_sha256": hashlib.sha256(json.dumps(before, sort_keys=True).encode()).hexdigest(),
                "changed": diff, "wrapper": java.read_text()}
    dst.with_name(dst.name + ".manifest.json").write_text(json.dumps(manifest, indent=1) + "\n")
    print(json.dumps({k: manifest[k] for k in ("files", "original_tree_sha256", "changed")}, indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
