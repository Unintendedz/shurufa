#!/usr/bin/env python3
"""版本表 `shared/contracts/editions.json` 的结构和跨字段约束。

单产品收敛后，仓库只保留未打包测试基线 `full` 与独立 macOS 产品 `shurufa`：
- 字段集合与 `shared/contracts/editions.schema.json` 声明一致，`platforms` 的六个键都在（非 macOS 段为空）；
- id 唯一且为 `["full", "shurufa"]`，显示名两两不同；
- 方案列表是 `crates/client-core/src/host_surface.rs` 中 `ALL_INPUT_SCHEMES` 的子集且保序；
- 资源组件互不重叠，并集恰好等于 `resources/desktop-dictionary.lock.json`；
- 生成的资源锁与 `scripts/editions.py gen-locks` 一致；
- macOS 身份标识、钥匙串服务与遥测目录互不冲突，且与 `EditionIdentity.h` 一致。
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
EDITIONS = ROOT / "shared/contracts/editions.json"
SCHEMA = ROOT / "shared/contracts/editions.schema.json"
HOST_SURFACE = ROOT / "crates/client-core/src/host_surface.rs"
DESKTOP_LOCK = ROOT / "resources/desktop-dictionary.lock.json"
LANGUAGE_LOCK = ROOT / "resources/language-dictionaries.lock.json"
INFO_PLIST = ROOT / "platforms/macos/Info.plist.in"
EDITION_IDENTITY = ROOT / "platforms/macos/src/core/EditionIdentity.h"

FULL = "full"
CHINESE_MAIN_SCHEMES = {"quanpin", "shuangpin", "wubi"}
CHINESE_ONLY_COMPONENTS = ["chinese-main", "ngram"]
ENGINE_TYPES = ROOT / "crates/engine/src/types.rs"
CHINESE_SCHEMES = {"quanpin", "shuangpin", "wubi", "cantonese", "zhuyin", "stroke"}
CLIENT_PREFERENCES = ROOT / "crates/client-core/src/preferences.rs"
CHINESE_FEATURES = ["offline_glosses", "handwriting"]
FEATURE_COMPONENTS = {"temporary_japanese": "japanese", "neural_keyboard": "sentence-model"}
SCHEME_LANGUAGE_DICTIONARIES = {"cantonese": "msime-cantonese.db", "zhuyin": "msime-zhuyin.db", "stroke": "msime-stroke.db"}
TAB_STROKE_FILTER_DICTIONARY = "msime-stroke.db"
TAB_STROKE_FILTER_SCHEMES = {"quanpin", "shuangpin"}
PREFERENCE_DEFAULT_SCHEMES = {"wubi_mixed_pinyin": "wubi"}

FULL_MACOS = {
    "input_method_bundle_id": "app.msime.inputmethod.MetasequoiaIME",
    "input_method_name": "水杉输入法",
    "settings_bundle_id": "app.msime.macos",
    "keychain_service": "com.metasequoia.msime.account",
    "cask": "msime",
    "dmg_prefix": "msime-macos",
}
FULL_VOICE_PROVIDER_SERVICE = "app.msime.client.voice.providers"
FULL_USAGE_REPORTING_DIRECTORY = "MSIME/telemetry"


def voice_provider_service(edition_id: str, section: dict) -> str:
    return FULL_VOICE_PROVIDER_SERVICE if edition_id == FULL else section["input_method_bundle_id"] + ".voice"


def usage_reporting_directory(edition_id: str) -> str:
    if edition_id == "shurufa":
        return "Shurufa/telemetry"
    return FULL_USAGE_REPORTING_DIRECTORY if edition_id == FULL else f"MSIME/{edition_id}/telemetry"


def load_generator():
    spec = importlib.util.spec_from_file_location("editions", ROOT / "scripts/editions.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def check_generated_locks(errors: list[str], table: dict) -> None:
    generator = load_generator()
    desktop_text = DESKTOP_LOCK.read_text(encoding="utf-8")
    desktop = json.loads(desktop_text)
    for problem in generator.drift(generator.generated_locks(table, desktop)):
        errors.append(f"{problem}; run python3 scripts/editions.py gen-locks")
    if generator.union_of_components(table, desktop) != desktop_text:
        errors.append(f"the union of resource_components does not reproduce {DESKTOP_LOCK.relative_to(ROOT)} byte for byte")


def camel_to_snake(name: str) -> str:
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


def engine_schemes(errors: list[str]) -> list[str]:
    text = HOST_SURFACE.read_text(encoding="utf-8")
    match = re.search(r"const ALL_INPUT_SCHEMES: \[InputScheme; \d+\] = \[(.*?)\];", text, re.S)
    if not match:
        errors.append(f"{HOST_SURFACE.relative_to(ROOT)}: ALL_INPUT_SCHEMES not found")
        return []
    return [camel_to_snake(name) for name in re.findall(r"InputScheme::(\w+)", match.group(1))]


def lock_names(path: pathlib.Path) -> list[str]:
    return [artifact["name"] for artifact in json.loads(path.read_text(encoding="utf-8"))["artifacts"]]


def plist_string(key: str) -> str | None:
    match = re.search(rf"<key>{key}</key>\s*<string>([^<]*)</string>", INFO_PLIST.read_text(encoding="utf-8"))
    return match.group(1) if match else None


def schema_keys(node: dict) -> set[str]:
    return set(node["properties"])


def check_keys(errors: list[str], where: str, value: object, expected: set[str], required: set[str]) -> bool:
    if not isinstance(value, dict):
        errors.append(f"{where}: expected an object")
        return False
    unknown = set(value) - expected
    missing = required - set(value)
    if unknown:
        errors.append(f"{where}: fields not declared in the schema: {sorted(unknown)}")
    if missing:
        errors.append(f"{where}: missing fields: {sorted(missing)}")
    return not unknown and not missing


def string_matches_schema(value: str, rule: dict) -> bool:
    if "const" in rule:
        return value == rule["const"]
    if "anyOf" in rule:
        return any(string_matches_schema(value, branch) for branch in rule["anyOf"])
    return "pattern" not in rule or re.search(rule["pattern"], value) is not None


def check_strings(errors: list[str], where: str, section: dict, node: dict) -> None:
    if not check_keys(errors, where, section, schema_keys(node), set(node["required"])):
        return
    for key, value in section.items():
        rule = node["properties"][key]
        minimum = rule.get("minLength", 1)
        if not isinstance(value, str) or len(value) < minimum:
            errors.append(f"{where}.{key}: expected a non-empty string")
        elif not string_matches_schema(value, rule):
            errors.append(f"{where}.{key}: {value!r} does not match the declared string constraints")


def check_schema_shape(errors: list[str], table: dict, schema: dict) -> None:
    check_keys(errors, "editions.json", table, schema_keys(schema), set(schema["required"]))
    edition = schema["$defs"]["edition"]
    for index, entry in enumerate(table.get("editions", [])):
        where = f"editions[{index}]"
        if not check_keys(errors, where, entry, schema_keys(edition), set(edition["required"])):
            continue
        for field in ["display_name", "preference_defaults", "resources", "features", "platforms"]:
            node = edition["properties"][field]
            check_keys(errors, f"{where}.{field}", entry[field], schema_keys(node), set(node.get("required", [])))
        if isinstance(entry["platforms"], dict):
            for platform, section in entry["platforms"].items():
                if section is not None and not isinstance(section, dict):
                    errors.append(f"{where}.platforms.{platform}: expected an object or null")
                elif platform != "macos" and section is not None:
                    errors.append(f"{where}.platforms.{platform}: 单产品收敛后非 macOS 平台段必须为 null")
            macos_section = entry["platforms"].get("macos")
            macos_node = edition["properties"]["platforms"]["properties"]["macos"]
            if isinstance(macos_section, dict):
                check_strings(errors, f"{where}.platforms.macos", macos_section, macos_node)


def check_main_dictionary_rule(errors: list[str]) -> None:
    text = ENGINE_TYPES.read_text(encoding="utf-8")
    match = re.search(r"pub const fn reads_main_dictionary\(self\) -> bool \{(.*?)\n    \}", text, re.S)
    if not match:
        errors.append(f"{ENGINE_TYPES.relative_to(ROOT)}: SchemeSet::reads_main_dictionary not found")
        return
    schemes = {camel_to_snake(name) for name in re.findall(r"SchemeType::(\w+)", match.group(1))}
    if schemes != CHINESE_MAIN_SCHEMES:
        errors.append(f"{ENGINE_TYPES.relative_to(ROOT)}: SchemeSet::reads_main_dictionary names {sorted(schemes)}, but this script's CHINESE_MAIN_SCHEMES is {sorted(CHINESE_MAIN_SCHEMES)}")


def check_chinese_scheme_rule(errors: list[str]) -> None:
    text = CLIENT_PREFERENCES.read_text(encoding="utf-8")
    match = re.search(r"pub fn of\(scheme: InputScheme\) -> Option<Self> \{(.*?)\n    \}", text, re.S)
    if not match:
        errors.append(f"{CLIENT_PREFERENCES.relative_to(ROOT)}: ChineseScheme::of not found")
        return
    schemes = {camel_to_snake(name) for name in re.findall(r"InputScheme::(\w+) => Some", match.group(1))}
    if schemes != CHINESE_SCHEMES:
        errors.append(f"{CLIENT_PREFERENCES.relative_to(ROOT)}: ChineseScheme::of names {sorted(schemes)}, but this script's CHINESE_SCHEMES is {sorted(CHINESE_SCHEMES)}")


def check_full(errors: list[str], full: dict, engine: list[str], components: dict) -> None:
    if full["input_schemes"] != engine:
        errors.append(f"edition full: input_schemes must be every engine scheme {engine}")
    if full["default_scheme"] != "quanpin":
        errors.append("edition full: default_scheme must stay quanpin")
    if full["preference_defaults"]:
        errors.append("edition full: preference_defaults must be empty")
    if sorted(full["resources"]["components"]) != sorted(components):
        errors.append("edition full: resources.components must include every component")
    for feature, enabled in full["features"].items():
        if enabled is not True:
            errors.append(f"edition full: features.{feature} must be true")
    display = plist_string("CFBundleDisplayName")
    if full["display_name"]["zh-Hans"] != display:
        errors.append(f"edition full: display_name.zh-Hans must equal CFBundleDisplayName {display!r} in {INFO_PLIST.relative_to(ROOT)}")
    macos = full["platforms"].get("macos")
    if macos != FULL_MACOS:
        errors.append(f"edition full: platforms.macos must match baseline {FULL_MACOS}")
    else:
        bundle = plist_string("CFBundleIdentifier")
        if macos["input_method_bundle_id"] != bundle:
            errors.append(f"edition full: platforms.macos.input_method_bundle_id must equal CFBundleIdentifier {bundle!r}")
        executable = plist_string("CFBundleExecutable")
        if macos["input_method_name"] != executable:
            errors.append(f"edition full: platforms.macos.input_method_name must equal CFBundleExecutable {executable!r}")


def check_macos(errors: list[str], editions: list[dict]) -> None:
    sections = [(entry["id"], entry["platforms"].get("macos")) for entry in editions]
    sections = [(edition_id, section) for edition_id, section in sections if section is not None]
    for key in FULL_MACOS:
        seen: dict[str, str] = {}
        for edition_id, section in sections:
            folded = section[key].casefold()
            if folded in seen:
                errors.append(f"editions {seen[folded]} and {edition_id}: platforms.macos.{key} {section[key]!r} is not unique")
            seen[folded] = edition_id
    bundles = [(edition_id, section["input_method_bundle_id"].casefold()) for edition_id, section in sections]
    for edition_id, bundle in bundles:
        for other_id, other in bundles:
            if edition_id != other_id and bundle.startswith(other + "."):
                errors.append(f"edition {edition_id}: platforms.macos.input_method_bundle_id collides with {other_id}")
    identifiers: dict[str, str] = {}
    for edition_id, section in sections:
        for key in ["input_method_bundle_id", "settings_bundle_id"]:
            folded = section[key].casefold()
            if folded in identifiers and identifiers[folded] != f"{edition_id}.{key}":
                errors.append(f"edition {edition_id}: platforms.macos.{key} {section[key]!r} is also {identifiers[folded]}")
            identifiers[folded] = f"{edition_id}.{key}"
    services: dict[str, str] = {}
    for edition_id, section in sections:
        for service in [section["keychain_service"], section["keychain_service"] + ".refresh", voice_provider_service(edition_id, section)]:
            folded = service.casefold()
            if folded in services:
                errors.append(f"edition {edition_id}: keychain service {service!r} is also used by {services[folded]}")
            services[folded] = edition_id
    identity = EDITION_IDENTITY.read_text(encoding="utf-8")
    for fragment in [
        f'MSIMEFullVoiceProviderKeychainService = @"{FULL_VOICE_PROVIDER_SERVICE}"',
        f'MSIMEFullUsageReportingDirectoryName = @"{FULL_USAGE_REPORTING_DIRECTORY}"',
        'stringByAppendingString:@".voice"',
        'if (MSIMEEditionIsShurufaIn(info)) return @"Shurufa/telemetry";',
    ]:
        if fragment not in identity:
            errors.append(f"{EDITION_IDENTITY.relative_to(ROOT)} no longer contains {fragment!r}")
    directories = [(edition_id, usage_reporting_directory(edition_id).casefold()) for edition_id, _ in sections]
    for edition_id, directory in directories:
        for other_id, other in directories:
            if edition_id != other_id and (directory == other or directory.startswith(other + "/")):
                errors.append(f"edition {edition_id}: usage reporting directory {directory!r} is inside {other_id}'s {other!r}")


def check_editions(errors: list[str], table: dict) -> None:
    engine = engine_schemes(errors)
    desktop = lock_names(DESKTOP_LOCK)
    language = set(lock_names(LANGUAGE_LOCK))

    if table.get("schema_version") != 1:
        errors.append(f"schema_version: expected 1, found {table.get('schema_version')!r}")

    components: dict[str, list[str]] = table.get("resource_components", {})
    seen_files: dict[str, str] = {}
    for component, files in components.items():
        for name in files:
            if name in seen_files:
                errors.append(f"resource_components: {name} is in both {seen_files[name]} and {component}")
            seen_files[name] = component
            if name not in desktop:
                errors.append(f"resource_components.{component}: {name} is not in {DESKTOP_LOCK.relative_to(ROOT)}")
    for name in desktop:
        if name not in seen_files:
            errors.append(f"resource_components: {DESKTOP_LOCK.relative_to(ROOT)} entry {name} belongs to no component")

    editions = table.get("editions", [])
    ids = [entry.get("id") for entry in editions]
    if ids != ["full", "shurufa"]:
        errors.append(f"editions: expected ['full', 'shurufa'], found {ids!r}")
    for language_tag in ["zh-Hans", "en"]:
        names = [entry["display_name"][language_tag] for entry in editions]
        for name in sorted({n for n in names if names.count(n) > 1}):
            errors.append(f"editions: display_name.{language_tag} {name!r} is used by more than one edition")

    for entry in editions:
        edition_id = entry["id"]
        where = f"edition {edition_id}"
        schemes = entry["input_schemes"]
        if not schemes:
            errors.append(f"{where}: input_schemes is empty")
        if len(set(schemes)) != len(schemes):
            errors.append(f"{where}: input_schemes repeats a scheme")
        unknown = [s for s in schemes if s not in engine]
        if unknown:
            errors.append(f"{where}: input_schemes not offered by the engine: {unknown}")
        elif schemes != [s for s in engine if s in schemes]:
            errors.append(f"{where}: input_schemes must follow the ALL_INPUT_SCHEMES order {engine}")
        if entry["default_scheme"] not in schemes:
            errors.append(f"{where}: default_scheme {entry['default_scheme']!r} is not in input_schemes")

        for key, value in entry["preference_defaults"].items():
            if not isinstance(value, bool):
                errors.append(f"{where}: preference_defaults.{key} must be a boolean")
            scheme = PREFERENCE_DEFAULT_SCHEMES.get(key)
            if scheme is not None and scheme not in schemes:
                errors.append(f"{where}: preference_defaults.{key} has no effect without {scheme}")

        chosen = entry["resources"]["components"]
        if len(set(chosen)) != len(chosen):
            errors.append(f"{where}: resources.components repeats a component")
        for component in chosen:
            if component not in components:
                errors.append(f"{where}: resources.components names unknown component {component!r}")
        if "core" not in chosen:
            errors.append(f"{where}: resources.components must include core")
        if CHINESE_MAIN_SCHEMES & set(schemes):
            if "chinese-main" not in chosen and "chinese-pinyin" not in chosen:
                errors.append(f"{where}: {sorted(CHINESE_MAIN_SCHEMES & set(schemes))} read msime-pinyin.db, so resources.components must include chinese-main or chinese-pinyin")
        if "japanese" in schemes and "japanese" not in chosen:
            errors.append(f"{where}: the japanese scheme needs the japanese component")
        for feature, component in FEATURE_COMPONENTS.items():
            enabled = entry["features"][feature]
            if not isinstance(enabled, bool):
                errors.append(f"{where}: features.{feature} must be a boolean")
            elif enabled and component not in chosen:
                errors.append(f"{where}: features.{feature} needs the {component} component")
        if "japanese" in chosen and not entry["features"]["temporary_japanese"] and "japanese" not in schemes:
            errors.append(f"{where}: ships the japanese component without using it")
        if "sentence-model" in chosen and not entry["features"]["neural_keyboard"]:
            errors.append(f"{where}: ships the sentence-model component without using it")
        writes_chinese = bool(CHINESE_SCHEMES & set(schemes))
        offers_chinese_extras = writes_chinese and edition_id != "shurufa"
        for feature in CHINESE_FEATURES:
            enabled = entry["features"][feature]
            if not isinstance(enabled, bool):
                errors.append(f"{where}: features.{feature} must be a boolean")
            elif enabled != offers_chinese_extras:
                errors.append(f"{where}: features.{feature} must be {str(offers_chinese_extras).lower()} for this edition")

        dictionaries = entry["language_dictionaries"]
        for name in dictionaries:
            if name not in language:
                errors.append(f"{where}: language_dictionaries names {name!r}, which is not in {LANGUAGE_LOCK.relative_to(ROOT)}")
        needed = {SCHEME_LANGUAGE_DICTIONARIES[s] for s in schemes if s in SCHEME_LANGUAGE_DICTIONARIES}
        tab_stroke_filter = entry["features"]["tab_stroke_filter"]
        if not isinstance(tab_stroke_filter, bool):
            errors.append(f"{where}: features.tab_stroke_filter must be a boolean")
        elif tab_stroke_filter:
            needed.add(TAB_STROKE_FILTER_DICTIONARY)
            if not TAB_STROKE_FILTER_SCHEMES & set(schemes):
                errors.append(f"{where}: features.tab_stroke_filter needs quanpin or shuangpin")
        if set(dictionaries) != needed:
            errors.append(f"{where}: language_dictionaries must be exactly {sorted(needed)}, found {sorted(dictionaries)}")

    check_main_dictionary_rule(errors)
    check_chinese_scheme_rule(errors)
    full = next((entry for entry in editions if entry["id"] == FULL), None)
    if full is not None:
        check_full(errors, full, engine, components)
    check_macos(errors, editions)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--editions", type=pathlib.Path, default=EDITIONS)
    args = parser.parse_args()

    table = json.loads(args.editions.read_text(encoding="utf-8"))
    schema = json.loads(SCHEMA.read_text(encoding="utf-8"))

    errors: list[str] = []
    check_schema_shape(errors, table, schema)
    if not errors:
        check_editions(errors, table)
    if not errors:
        check_generated_locks(errors, table)

    if errors:
        for error in errors:
            print(f"FAIL: {error}")
        return 1
    print(f"editions: {len(table['editions'])} editions agree with the schema, the engine schemes and the resource locks")
    return 0


if __name__ == "__main__":
    sys.exit(main())
