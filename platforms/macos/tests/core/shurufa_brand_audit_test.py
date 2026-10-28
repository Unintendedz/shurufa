#!/usr/bin/env python3
"""图标审计必须检查固定的 13 项，空清单或缺项不能通过。"""
import hashlib
import importlib.util
import tempfile
from pathlib import Path

script = Path(__file__).resolve().parents[2] / "scripts/audit_shurufa_brand.py"
spec = importlib.util.spec_from_file_location("brand_audit", script)
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)
with tempfile.TemporaryDirectory(prefix="shurufa-icon-audit-") as root:
    bundle = Path(root)
    resources = bundle / "Contents/Resources"
    resources.mkdir(parents=True)
    manifest = {}
    for name in audit.ICON_NAMES:
        data = ("synthetic:" + name).encode()
        (resources / name).write_bytes(data)
        manifest[name] = hashlib.sha256(data).hexdigest()
    assert len(manifest) == 13
    assert audit.audit_icons(bundle, manifest) == ([], 13)
    failures, count = audit.audit_icons(bundle, {})
    assert len(failures) == 13 and count == 0
    missing = dict(manifest)
    missing.pop("MSIMEClientInputMethod.icns")
    failures, count = audit.audit_icons(bundle, missing)
    assert len(failures) == 1 and count == 12
    (resources / "MSIMEClientInputMethod.icns").write_bytes(b"synthetic changed")
    failures, count = audit.audit_icons(bundle, manifest)
    assert len(failures) == 1 and count == 12
print("PASS: 13 个图标、空清单、缺项和像素哈希变更")
