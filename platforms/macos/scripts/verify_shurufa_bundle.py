#!/usr/bin/env python3
"""核对实际产物的标识、架构、模型和运行时路径；不读取或输出凭据。"""
import hashlib
import json
import plistlib
import subprocess
import sys
from pathlib import Path

bundle = Path(sys.argv[1]).resolve()
contents = bundle / "Contents"
info = plistlib.loads((contents / "Info.plist").read_bytes())
identifier = "dev.shurufa.inputmethod.Shurufa"
assert info["CFBundleIdentifier"] == identifier
assert info["CFBundleDisplayName"] == "Shurufa"
assert info["InputMethodConnectionName"] == identifier + "_Connection"
assert info["MSIMEStateDirectoryName"] == "Shurufa"
assert info["MSIMESettingsBundleIdentifier"] == "dev.shurufa.settings"
assert info["MSIMEKeychainService"] == "dev.shurufa.account"
assert info["MSIMEInputSchemes"] == ["quanpin", "shuangpin"]
modes = sorted(info["ComponentInputModeDict"]["tsInputModeListKey"])
assert modes == sorted(identifier + "." + suffix for suffix in ("Hans", "Shuangpin"))
assert not any(key.startswith("SU") for key in info)
assert not (contents / "Frameworks/Sparkle.framework").exists()
executable = contents / "MacOS" / info["CFBundleExecutable"]
assert subprocess.check_output(["lipo", "-archs", str(executable)], text=True).strip() == "arm64"
linked = subprocess.check_output(["otool", "-L", str(executable)], text=True)
assert "Sparkle" not in linked
assert (contents / "MacOS/shurufa-voice-local").is_file()
assert not (contents / "MacOS/msime-voice-local").exists()
model = contents / "Resources/EngineResources/sentence-model.safetensors"
assert model.stat().st_size == 4486280
assert hashlib.sha256(model.read_bytes()).hexdigest() == "86ac529510cb3b4968a5e6ade83ec8080f5b34a0a681e75c362accbbd387d1c1"
identity = json.loads(subprocess.check_output([str(executable), "--describe-host"], text=True))
assert identity["bundle_id"] == identifier
assert identity["preferences_domain"] == identifier
assert identity["state_directory"] == str(Path.home() / "Library/Application Support/Shurufa")
assert identity["voice_keychain"] == identifier + ".voice"
assert identity["dictionary_notification"].endswith(".shurufa")
assert identity["statistics_notification"].endswith(".shurufa")
assert identity["voice_helper"] == "shurufa-voice-local"
print(json.dumps({"bundle": str(bundle), "modes": modes, "identity": identity,
    "architecture": "arm64", "sparkle": False, "neural_model_bytes": model.stat().st_size},
    ensure_ascii=False, indent=2))
