#!/usr/bin/env python3
"""各版本的 macOS 输入法 bundle 身份（platforms/macos/scripts/edition_bundle.py 的输出）。"""

from __future__ import annotations

import importlib.util
import pathlib
import plistlib
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
MACOS = ROOT / "platforms/macos"
TEMPLATE = MACOS / "Info.plist.in"
LOCALES = ["zh-Hans", "en"]
MODE_HEADER = MACOS / "src/input/InputModeIdentifiers.h"
NAMES_CHECK = MACOS / "tests/settings/info_plist_names.py"
LANGUAGE_EDITION_LANGUAGES = {"japanese": "ja", "vietnamese": "vi", "tibetan": "bo"}


def load_generator():
    spec = importlib.util.spec_from_file_location("edition_bundle", MACOS / "scripts/edition_bundle.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def strings_source(locale: str) -> str:
    return (MACOS / "resources" / f"{locale}.lproj" / "InfoPlist.strings").read_text(encoding="utf-8")


def check_full(errors: list[str], generator, table: dict) -> None:
    template = TEMPLATE.read_text(encoding="utf-8")
    if generator.info_plist(template, "full", table) != template:
        errors.append("the full Info.plist differs from Info.plist.in")
    for locale in LOCALES:
        if generator.info_plist_strings(strings_source(locale), "full", table) != strings_source(locale):
            errors.append(f"the full {locale} InfoPlist.strings differs from the source")
    if generator.marker("full") is not None:
        errors.append("the full settings app must not carry an edition declaration")


def check_edition(errors: list[str], generator, table: dict, entry: dict, others: list[str], directory: pathlib.Path) -> None:
    edition_id = entry["id"]
    macos = entry["platforms"]["macos"]
    bundle = macos["input_method_bundle_id"]
    where = f"edition {edition_id}"
    text = generator.info_plist(TEMPLATE.read_text(encoding="utf-8"), edition_id, table)
    try:
        plist = plistlib.loads(text.encode("utf-8"))
    except Exception as error:  # noqa: BLE001
        errors.append(f"{where}: the generated Info.plist is not a property list: {error}")
        return
    resources = directory / edition_id / "resources"
    strings: dict[str, str] = {}
    for locale in LOCALES:
        strings[locale] = generator.info_plist_strings(strings_source(locale), edition_id, table)
        target = resources / f"{locale}.lproj" / "InfoPlist.strings"
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(strings[locale], encoding="utf-8")
    plist_path = directory / edition_id / "Info.plist.in"
    plist_path.write_text(text, encoding="utf-8")

    expected = {
        "CFBundleIdentifier": bundle,
        "CFBundleExecutable": macos["input_method_name"],
        "InputMethodConnectionName": f"{bundle}_Connection",
    }
    if edition_id == "full":
        expected["CFBundleDisplayName"] = entry["display_name"]["zh-Hans"]
    else:
        expected.update({
            "CFBundleDisplayName": entry["display_name"]["zh-Hans"],
            "CFBundleName": entry["display_name"]["en"],
            "MSIMEEdition": edition_id,
            "MSIMEInputSchemes": entry["input_schemes"],
            "MSIMEDefaultScheme": entry["default_scheme"],
            "MSIMESettingsBundleIdentifier": macos["settings_bundle_id"],
            "MSIMEKeychainService": macos["keychain_service"],
        })
        if "state_directory_name" in macos:
            expected["MSIMEStateDirectoryName"] = macos["state_directory_name"]
        if "voice_helper_name" in macos:
            expected["MSIMELocalVoiceHelperName"] = macos["voice_helper_name"]
        if not entry["features"]["handwriting"]:
            expected["MSIMEHandwriting"] = False
    for key, value in expected.items():
        if plist.get(key) != value:
            errors.append(f"{where}: {key} is {plist.get(key)!r}, expected {value!r}")
    if edition_id == "full" and any(key.startswith("MSIME") for key in plist):
        errors.append(f"{where}: the full Info.plist must not declare an edition")

    plan = generator.mode_plan(entry)
    modes = plist["ComponentInputModeDict"]["tsInputModeListKey"]
    declared = {identifier[len(bundle) + 1:] for identifier in modes}
    if declared != set(plan):
        errors.append(f"{where}: declares modes {sorted(declared)}, expected {sorted(plan)}")
    if edition_id == "shurufa" and plist.get("TICapsLockLanguageSwitchCapable") is not True:
        errors.append(f"{where}: Shurufa 必须声明系统 Caps Lock 切换能力")
    if "Hans" not in declared:
        errors.append(f"{where}: 必须声明中文主模式 .Hans")
    if ("Roman" in declared) != (edition_id != "shurufa"):
        errors.append(f"{where}: Shurufa 不声明 .Roman，其他版本必须保留英文模式")
    visible = plist["ComponentInputModeDict"]["tsVisibleInputModeOrderedArrayKey"]
    if set(visible) != set(modes):
        errors.append(f"{where}: tsVisibleInputModeOrderedArrayKey {visible} does not list exactly the declared modes")
    template_modes = plistlib.loads(TEMPLATE.read_bytes())["ComponentInputModeDict"]["tsInputModeListKey"]
    full_bundle = generator.full_entry(table)["platforms"]["macos"]["input_method_bundle_id"]
    for suffix, source in plan.items():
        body = modes.get(f"{bundle}.{suffix}", {})
        if body.get("TISInputSourceID") != f"{bundle}.{suffix}":
            errors.append(f"{where}: mode {suffix} carries TISInputSourceID {body.get('TISInputSourceID')!r}")
        icon = template_modes[f"{full_bundle}.{source}"]["tsInputModeMenuIconFileKey"]
        if body.get("tsInputModeMenuIconFileKey") != icon:
            errors.append(f"{where}: mode {suffix} should show the {source} icon {icon}")
    primary = template_modes[f"{full_bundle}.{plan['Hans']}"]
    language = primary["TISIntendedLanguage"]
    for suffix in (("Hans",) if edition_id == "shurufa" else ("Hans", "Roman")):
        body = modes.get(f"{bundle}.{suffix}", {})
        if body.get("TISIntendedLanguage") != language:
            errors.append(f"{where}: mode {suffix} is filed under {body.get('TISIntendedLanguage')!r}, expected {language!r}")
    hans = modes.get(f"{bundle}.Hans", {})
    for key in ("tsInputModeCharacterRepertoireKey", "tsInputModeScriptKey"):
        if hans.get(key) != primary[key]:
            errors.append(f"{where}: the primary mode's {key} is {hans.get(key)!r}, expected {primary[key]!r}")
    if hans.get("tsInputModeDefaultStateKey") is not True:
        errors.append(f"{where}: the primary mode must be enabled on install")

    for other in others:
        if other == bundle:
            continue
        if other in text:
            errors.append(f"{where}: the Info.plist mentions {other}, another edition's bundle id")
        for locale in LOCALES:
            if other in strings[locale]:
                errors.append(f"{where}: the {locale} InfoPlist.strings mentions {other}, another edition's bundle id")

    result = subprocess.run(
        [sys.executable, str(NAMES_CHECK), str(plist_path), str(resources), str(ROOT)],
        capture_output=True, text=True, check=False,
    )
    if result.returncode != 0:
        for line in result.stderr.strip().splitlines():
            errors.append(f"{where}: {line}")


def check_mode_suffixes(errors: list[str], generator) -> None:
    header = MODE_HEADER.read_text(encoding="utf-8")
    for suffix in set(generator.SCHEME_MODES.values()) | {generator.ENGLISH}:
        if f'MSIMEInputModeIdentifier(@"{suffix}")' not in header:
            errors.append(f"{MODE_HEADER.relative_to(ROOT)} has no mode for the suffix {suffix} the generator uses")


def main() -> int:
    generator = load_generator()
    table = generator.load_table()
    editions = [entry for entry in table["editions"] if entry["platforms"].get("macos") is not None]
    others = [entry["platforms"]["macos"]["input_method_bundle_id"] for entry in editions]
    errors: list[str] = []
    check_full(errors, generator, table)
    check_mode_suffixes(errors, generator)
    with tempfile.TemporaryDirectory() as directory:
        for entry in editions:
            check_edition(errors, generator, table, entry, others, pathlib.Path(directory))
    if errors:
        for error in errors:
            print(f"FAIL: {error}")
        return 1
    print(f"macos editions: {len(editions)} editions generate distinct, well-formed bundles and full is unchanged")
    return 0


if __name__ == "__main__":
    sys.exit(main())
