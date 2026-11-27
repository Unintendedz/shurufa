#!/usr/bin/env python3
"""校验 Tier 1 端侧神经解码护栏评测集结构、合成数据声明与本地门禁挂载契约。"""

from __future__ import annotations

import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
GUARDRAILS = ROOT / "resources/eval/shurufa-tier1-guardrails-v1.tsv"
EVAL_SCRIPT = ROOT / "scripts/eval-tier1.sh"
VERIFY_LOCAL = ROOT / "scripts/verify-local.sh"
TIER1_EXAMPLE = ROOT / "crates/input-runtime/examples/tier1_eval.rs"

REQUIRED_COHORTS = {
    "boundary-single": 144,
    "phantom-reading": 12,
    "unmeasured-dict": 12,
    "typo-genuine": 18,
    "mixed-english-ctx": 10,
    "oov-2char": 10,
}


def main() -> int:
    assert GUARDRAILS.is_file(), f"missing {GUARDRAILS}"
    assert EVAL_SCRIPT.is_file(), f"missing {EVAL_SCRIPT}"
    assert TIER1_EXAMPLE.is_file(), f"missing {TIER1_EXAMPLE}"

    lines = GUARDRAILS.read_text(encoding="utf-8").splitlines()
    assert any("100% 合成数据" in line for line in lines[:5]), "missing synthetic-only header declaration"

    seen_ids: set[str] = set()
    cohort_counts: dict[str, int] = {k: 0 for k in REQUIRED_COHORTS}
    forbidden_rows = 0
    header_seen = False

    for lineno, line in enumerate(lines, start=1):
        if not line or line.startswith("#"):
            continue
        if not header_seen:
            assert line == "id\tinput\tgold\tsyllables\ttags\tcontext\tforbidden_page1", (
                f"unexpected header at line {lineno}: {line!r}"
            )
            header_seen = True
            continue
        parts = line.split("\t")
        assert 6 <= len(parts) <= 7, f"line {lineno}: expected 6 or 7 tab columns, got {len(parts)}"
        cid, inp, gold, syl_str, tags_str = parts[0], parts[1], parts[2], parts[3], parts[4]
        assert cid and cid not in seen_ids, f"line {lineno}: duplicate or empty id {cid!r}"
        seen_ids.add(cid)
        assert inp.isascii() and inp.isalpha() and inp.islower(), (
            f"line {lineno} ({cid}): input must be lowercase ASCII letters, got {inp!r}"
        )
        assert gold, f"line {lineno} ({cid}): empty gold"
        syl = int(syl_str)
        assert syl >= 1, f"line {lineno} ({cid}): invalid syllables {syl}"
        tags = {t.strip() for t in tags_str.split(",") if t.strip()}
        for cohort in REQUIRED_COHORTS:
            if cohort in tags:
                cohort_counts[cohort] += 1
        if len(parts) == 7 and parts[6].strip():
            forbidden_rows += 1

    assert header_seen, "missing TSV header row"
    for cohort, min_count in REQUIRED_COHORTS.items():
        actual = cohort_counts[cohort]
        assert actual >= min_count, f"cohort {cohort} has {actual} cases, expected >= {min_count}"
    assert forbidden_rows >= 15, f"expected >= 15 rows with forbidden_page1 guards, got {forbidden_rows}"

    verify_text = VERIFY_LOCAL.read_text(encoding="utf-8")
    assert "scripts/eval-tier1.sh" in verify_text, "verify-local.sh must invoke scripts/eval-tier1.sh"

    print(
        f"tier1 eval suite contract ok: {len(seen_ids)} cases across {len(REQUIRED_COHORTS)} cohorts "
        f"({forbidden_rows} forbidden-page1 guards)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
