#!/usr/bin/env python3
"""按 plist 结构、Mach-O 节和资源路径列出产品包的品牌残留，不用正则猜测字符串含义。"""
import argparse
import collections
import hashlib
import json
import plistlib
import struct
import subprocess
from pathlib import Path

TOKENS = ("水杉", "Metasequoia", "MSIME", "msime.app", "github.com/metasequoiaime", "t.me/msimegroup")


def sections(data):
    if data[:4] != b"\xcf\xfa\xed\xfe":
        return []
    count = struct.unpack_from("<I", data, 16)[0]
    position = 32
    result = []
    segments = []
    for _ in range(count):
        command, size = struct.unpack_from("<II", data, position)
        if command == 0x19:
            segment_name = data[position + 8:position + 24].split(b"\0", 1)[0].decode()
            file_offset, file_length = struct.unpack_from("<QQ", data, position + 40)
            if file_length:
                segments.append((file_offset, file_offset + file_length, segment_name))
            number = struct.unpack_from("<I", data, position + 64)[0]
            for index in range(number):
                start = position + 72 + index * 80
                name = data[start:start + 16].split(b"\0", 1)[0].decode()
                segment = data[start + 16:start + 32].split(b"\0", 1)[0].decode()
                length, offset = struct.unpack_from("<QI", data, start + 40)
                if offset and length:
                    result.append((offset, offset + length, segment + "," + name))
        elif command == 2:
            _, _, offset, length = struct.unpack_from("<IIII", data, position + 8)
            result.append((offset, offset + length, "symbol-string-table"))
        position += size
    return result + [(0, position, "load-commands")] + segments


def metadata_strings(value, trail=""):
    if isinstance(value, dict):
        for key, child in value.items():
            yield from metadata_strings(child, trail + "/" + str(key))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield from metadata_strings(child, trail + "/" + str(index))
    elif isinstance(value, str):
        yield trail, value


def audit(bundle):
    hits = []
    visible_failures = []
    icon_manifest = json.loads((Path(__file__).parents[1] / "resources/shurufa/icon-sha256.json").read_text())
    icon_failures = []
    for name, expected in icon_manifest.items():
        icon = bundle / "Contents/Resources" / name
        actual = hashlib.sha256(icon.read_bytes()).hexdigest() if icon.is_file() else None
        if actual != expected:
            icon_failures.append({"path":name, "expected":expected, "actual":actual})
    for file in sorted(bundle.rglob("*")):
        if not file.is_file():
            continue
        path = file.relative_to(bundle).as_posix()
        data = file.read_bytes()
        ranges = sections(data)
        if file.suffix in (".plist", ".strings"):
            try:
                values = plistlib.loads(data)
            except plistlib.InvalidFileException:
                values = json.loads(subprocess.check_output(["plutil", "-convert", "json", "-o", "-", str(file)], text=True))
            if values is not None:
                for key, value in metadata_strings(values):
                    if not any(token in value for token in TOKENS):
                        continue
                    leaf = key.rsplit("/", 1)[-1]
                    visible = file.suffix == ".strings" or leaf in ("CFBundleName", "CFBundleDisplayName") or leaf.endswith("UsageDescription")
                    if visible:
                        visible_failures.append({"path": path, "field": key, "value": value})
        for token in TOKENS:
            for encoding in ("utf-8", "utf-16-le"):
                needle = token.encode(encoding)
                offset = data.find(needle)
                while offset >= 0:
                    section = next((name for begin, end, name in ranges if begin <= offset < end), "")
                    if path.startswith("Contents/Resources/EngineResources/"):
                        reason = "输入词库、词频或模型资源；其中的树名和项目词条不是界面品牌"
                    elif path.startswith("Contents/Resources/Licenses/") or file.name == "LICENSE-TextStream":
                        reason = "保留的第三方许可和来源声明"
                    elif section in ("symbol-string-table", "__LINKEDIT", "load-commands", "__TEXT,__objc_classname", "__TEXT,__objc_methname", "__TEXT,__objc_methtype"):
                        reason = "链接符号或 Objective-C 运行时标识；按要求不重命名"
                    elif section:
                        reason = "共享二进制的常量、日志或代码；保留其他 edition/不可达功能模板，Shurufa 展示由身份转换和原生控件检查验证"
                    elif path == "Contents/Info.plist" or path.startswith("Contents/_CodeSignature/"):
                        reason = "bundle 内部类名、资源文件名或签名清单；显示名和权限说明另按字段检查"
                    elif path.startswith("Contents/Resources/textstream/"):
                        reason = "本地语音辅助脚本和协议中的内部兼容标识"
                    elif file.suffix == ".tiff" and path.startswith("Contents/Resources/"):
                        reason = "TIFF 图标元数据保留生成前的 PNG 文件名；不是像素内文字"
                    elif file.name == "plugin.toml" and path.startswith("Contents/Resources/sound-packs/"):
                        reason = "音效包 author 字段的来源署名；原生设置不展示作者，保留第三方来源"
                    else:
                        reason = "资源内残留；需结合此路径的用途逐项核查"
                    before, after = (80, 160) if encoding == "utf-8" else (80, 160)
                    start = max(0, offset - before)
                    if encoding == "utf-16-le":
                        start += (offset - start) % 2
                    excerpt = data[start:min(len(data), offset + after)].decode(encoding, errors="replace")
                    hits.append({"path":path, "offset":offset, "section":section, "encoding":encoding,
                        "token":token, "reason":reason, "excerpt":excerpt})
                    offset = data.find(needle, offset + len(needle))
    return {"bundle":str(bundle), "visible_metadata_failures":visible_failures,
        "icon_hash_failures":icon_failures, "verified_icon_count":len(icon_manifest) - len(icon_failures),
        "counts_by_reason":dict(collections.Counter(hit["reason"] for hit in hits)), "hits":hits}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("bundle", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    report = audit(args.bundle.resolve())
    args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2))
    print(json.dumps({key:value for key,value in report.items() if key != "hits"}, ensure_ascii=False, indent=2))
    return bool(report["visible_metadata_failures"] or report["icon_hash_failures"])


if __name__ == "__main__":
    raise SystemExit(main())
