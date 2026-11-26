#!/usr/bin/env python3
"""从 resources/language-dictionaries.lock.json 下载并校验轻量 Tab 笔画筛选词库 `msime-stroke.db` 及其许可证。

默认输出到 `target/language-dictionaries/`；若本地已有字节大小与 SHA-256 均匹配的文件则直接跳过下载，保证构建可重复且防篡改。
"""
import argparse
import hashlib
import json
import sys
import tempfile
from pathlib import Path

import download_retry

ROOT = Path(__file__).resolve().parents[1]
LOCK = ROOT / "resources/language-dictionaries.lock.json"
DEFAULT_OUT = ROOT / "target/language-dictionaries"
STROKE_ARTIFACTS = {"msime-stroke.db", "msime-rime_stroke_LICENSE.txt"}
CHUNK = 1 << 20


def digest(path: Path) -> str:
    sha = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(CHUNK), b""):
            sha.update(block)
    return sha.hexdigest()


def fetch(artifact: dict, destination: Path) -> None:
    url = artifact["url"]
    if not url.startswith("https://"):
        raise SystemExit(f"{artifact['name']}: refusing a non-HTTPS source")
    print(f"  fetching {artifact['name']} ({artifact['size'] / 1e6:.2f} MB)", file=sys.stderr)
    staged = tempfile.NamedTemporaryFile(dir=destination.parent, delete=False)
    staged_path = Path(staged.name)
    try:
        with staged:
            with download_retry.urlopen(url, timeout=300) as response:
                advertised = response.headers.get("Content-Length")
                if advertised is not None:
                    try:
                        if int(advertised) > artifact["size"]:
                            raise SystemExit(f"{artifact['name']}: response is larger than the lock")
                    except ValueError:
                        pass
                size = 0
                while block := response.read(CHUNK):
                    size += len(block)
                    if size > artifact["size"]:
                        raise SystemExit(f"{artifact['name']}: response is larger than the lock")
                    staged.write(block)
    except BaseException:
        staged_path.unlink(missing_ok=True)
        raise
    actual = digest(staged_path)
    size = staged_path.stat().st_size
    if actual != artifact["sha256"] or size != artifact["size"]:
        staged_path.unlink(missing_ok=True)
        raise SystemExit(
            f"{artifact['name']}: expected {artifact['sha256']} at {artifact['size']} bytes, got {actual} at {size}"
        )
    staged_path.replace(destination)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    arguments = parser.parse_args()
    if not LOCK.is_file():
        raise SystemExit(f"missing lock file: {LOCK}")
    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    selected = [artifact for artifact in lock["artifacts"] if artifact["name"] in STROKE_ARTIFACTS]
    if {artifact["name"] for artifact in selected} != STROKE_ARTIFACTS:
        raise SystemExit(f"{LOCK} does not pin all required stroke artifacts: {sorted(STROKE_ARTIFACTS)}")
    arguments.out.mkdir(parents=True, exist_ok=True)
    for artifact in selected:
        destination = arguments.out / artifact["name"]
        if (
            destination.is_file()
            and destination.stat().st_size == artifact["size"]
            and digest(destination) == artifact["sha256"]
        ):
            print(f"  {artifact['name']}: already at the locked digest", file=sys.stderr)
            continue
        fetch(artifact, destination)
    print(arguments.out)


if __name__ == "__main__":
    main()
