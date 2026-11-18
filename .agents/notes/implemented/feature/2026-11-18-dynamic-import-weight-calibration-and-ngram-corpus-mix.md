# Agent Note: 动态标定无权重导词与自造词初始权重并支持口语 N 元语料混入

Status: implemented

## Problem

分析随包发布的 `msime-pinyin.db` 词频分布可知，低频和中小频同音表里大量内置词汇的权重低于历史默认插入常量 `INSERTED_WEIGHT = 10_000`：`tbl_2_z` 有 70%（8,615 / 12,380）、`tbl_3_z` 有 88%（18,916 / 21,392）、`tbl_4_z` 有 92%（25,479 / 27,680）的词条权重小于 `10_000`。由此带来两层数据层失真：

1. **无权重导词与首次自造词直接越权篡位**：用户导入未带词频列或省略权重的词库（纯文本词表、无权重 TSV、`100%` 缺省权重以外的无计数行、SCEL 搜狗细胞词库、`parse_hans` 仅提供汉字的批量词表、Gboard v1 导出文本），或首次上屏组出全拼自造词触发 `QuanpinDictionary::insert_new_word` 时，一律写入固定权重 `10_000`，在同一拼音键原本最高权重仅数千的情况下立刻空降首位压过固有词条；且若直到最终落库或仅在部分入口标定，`personal.rs` 按原 `weight=?4` 删改个人词库或集合 `state` / `outbox.json` 重放时会产生权重失配。同时，在 `insert_new_word` 中若拆成“查重 + 查最大权重”两条 SQL 语句，会在首次上屏热点路径上多出 3 次堆分配，破坏九键首次上屏缓存零多余分配预算（`<= 32`）。
2. **离线 N 元表只受维基百科书面语料支配**：`msime-dict-builder` 的 `Ngram` 阶段先前仅从固定 `zhwiki` dump 与拼音表生成 `msime-bigram.bin` 和 `msime-trigram.bin`，缺乏一条在不改写既有发布锁定资源的前提下、按显式倍率混入现代日常口语搭配纯文本语料的受控通道。

## Decision

1. **同音键动态标定无权重拼音词频（保留显式权重契约）**：
   - 在 `PinyinDatabase`（[`crates/engine/src/dictionary/pinyin.rs`](../../../../crates/engine/src/dictionary/pinyin.rs)）提供只读安全打开 `open_readonly`、单键峰值查询 `max_weight`、`calibrated_insert_weight`，以及把“查重 `(key, value)` 是否已存在”与“读取同键既有最高权重”合进一条聚合查询（`SELECT MAX(CASE WHEN value=?2 THEN weight END), MAX(weight)`）的 `weight_for_new_word`。当同全拼键已有最高权重 `existing_max <= 10_000` 时标定为 `(existing_max - 1).max(1)`；当键尚无内置词条或既有最高权重本就大于 `10_000` 时沿用 `10_000`（保证空键单测与常见高频词区间行为不变）。
   - `QuanpinDictionary::insert_new_word`（[`crates/engine/src/quanpin/dictionary.rs`](../../../../crates/engine/src/quanpin/dictionary.rs)）改用 `weight_for_new_word` 单次查表并通过 `insert_word_with_weight` 与 `record_user_insert` 落盘，既完成动态标定又不增加任何额外 SQL 预编译与堆分配。
   - 宿主读取器端口通过 [`crates/engine/src/host/dictionary.rs`](../../../../crates/engine/src/host/dictionary.rs) 暴露 `PinyinWeightCalibrator` 并导出到 [`crates/engine/src/host/mod.rs`](../../../../crates/engine/src/host/mod.rs)，由 [`crates/host-api/src/dictionary.rs`](../../../../crates/host-api/src/dictionary.rs) 中的 `EngineReadings` 以 `OnceLock<PinyinWeightCalibrator>` 懒加载共享只读连接；并在 `ImportEntry`（[`crates/client-services/src/dictionary/import.rs`](../../../../crates/client-services/src/dictionary/import.rs)）增设 `#[serde(default)] pub defaulted_weight: bool` 区分「调用方显式指定 10000」与「格式未提供权重而回退默认值」。
   - 在共享解析边界 `HanReadings::calibrated_pinyin_weight`、`collections::import_scel_with_readings`、`collections::parse_rows` 与 `collections::parse_hans`（[`crates/client-services/src/dictionary/collections.rs`](../../../../crates/client-services/src/dictionary/collections.rs)）以及 `host_api::dictionary::{parse_import, parse_hans_import}` 统一执行前置标定，保证个人词库条目、集合 `state`、`outbox.json`、`personal_journal` 与 `tbl_*` 四处存储的权重严格一致。显式权重导词与 Rime 百分比换算仍保留原始权重。

2. **离线构建器支持受控混入口语纯文本 N 元语料（不触碰当前锁定发布哈希）**：
   - `Counts::count_plain_corpus`（[`crates/dict-builder/src/ngram.rs`](../../../../crates/dict-builder/src/ngram.rs)）经 `private_file::open_read_only` 安全读取外部纯文本（拒绝符号链接与特殊文件），按行切分汉字连续片段以既有拼音词汇表前向最大匹配切词统计，并将命中 `unigram` / `bigram` / `trigram` 计数乘以 `weight` 叠加进总数；跳过 `#` 注释与空行。
   - `msime-dict-build` 顶层 CLI 与 `Ngram` 阶段（[`crates/dict-builder/src/main.rs`](../../../../crates/dict-builder/src/main.rs)、[`crates/dict-builder/README.md`](../../../../crates/dict-builder/README.md)）新增可重复参数 `--extra-ngram-corpus <PATH[:WEIGHT]>`（默认倍率为 `1`）。未传该参数时生成的 `msime-bigram.bin` / `msime-trigram.bin` 与原逻辑逐字节一致，不破坏当前 `resources/desktop-dictionary.lock.json` 与 `resources/editions/shurufa.lock.json` 的锁定校验。
   - 在 [`resources/dictionary-sources/cn/conversational-ngram-seed.txt`](../../../../resources/dictionary-sources/cn/conversational-ngram-seed.txt) 沉淀首批完全由合成通用例句组成的 CC0 口语搭配种子文本（含常见动宾、定中与高频日常短句，严禁写入任何真实用户输入或隐私文本），供后续词库独立重新构建和迭代扩充。

## Historical audit

- [超高频核心词渐进调频护城河与语境同音竞争解耦](../bug-fix/2026-11-18-core-word-frequency-moat.md)：上下游互补并互链。前者防止已有词被单次误选越级击穿，本篇防止无权重导词和自造词从源头写入 `10_000` 虚高初始权重并补充离线语料渠道。
- [导入 Rime 百分比权重时保留合法词条](../bug-fix/2026-10-18-rime-percentage-weight.md)：部分重叠并互链。明月拼音百分比等显式权重的换算基数与比例语义保持不变；仅对未携带权重的默认行走同键最高词频标定。
- [Gboard v1 文本词库复用共享导入链路](2026-10-20-gboard-dictionary-import.md)：事实更新并互链。Gboard v1 文本第三列仅为语言标签，进入 `ImportEntry` 时标记 `defaulted_weight = true`，写入拼音个人词库或集合前由原先一律 `10000` 收敛为「空键 `10000`，低频同音键压在 `(existing_max - 1).max(1)`」。
- [词网格不再因为负的二元分扣分](../bug-fix/2026-10-13-lattice-negative-bigram.md)：上下文相关并互链。维基书面语料造成的共现缺失与负分偏置，除了在解码侧截断负二元分，还可通过 `--extra-ngram-corpus` 在离线 rebuild 时补充口语共现。

## Alternatives considered

- **把个人导入/自造词全部统一固定成更小的全局常量（如 `500` 或 `100`）**：实现只需改一个常量，但不区分低频生僻键与本无内置词的空键，且会把所有既有空键导入测试和用户期待新专名「无冲突时即可上屏」的手感一起压坏。按同全拼键现存最高权重动态封顶既保空键行为，又杜绝压过低频内置词。
- **只在最终写库层做标定，不把 `calibrated_pinyin_weight` 前置于解析与集合快照**：表面改动最少，但会让集合缓存 JSON、`outbox.json`、`personal_journal` 与真实写进 SQLite `tbl_*` 的权重不一致，导致 `personal.rs` 按 `weight=?4` 删除或修改条目时找不到行。
- **直接修改并重签发本地 `EngineResources` 中的 `msime-bigram.bin` 覆盖发布产物**：`stage-resources.sh` 与 `verify-resources` 会核对发布产物与锁定 JSON 的 SHA-256，本仓单次本地提交不重新打包签发完整词库发布包，故通过 `--extra-ngram-corpus` 与种子语料把能力接入正统构建流水线，并在运行时由万象 `.gram` 与静态先验护栏共同覆盖。

## Consequences

- **收益**：导词（SCEL / Gboard / 无权重 TSV / `parse_hans`）和全拼自造词在面对低频同音键时不再空降第 1 挤掉已有词条；九键首次上屏保持零额外堆分配；显式填写了词频的导入与空键导入保持完全兼容；离线词库构建获得了无副作用的口语语料混入入口，不传参数时默认输出零漂移。
- **代价与边界**：当目标 `msime-pinyin.db` 中同全拼键的固有最高词恰好是极低权重（例如 `1`）时，无权重导词与新造词只能同权落于 `1`（由后续调频累积区分）；随包已发布的 `msime-bigram.bin` / `msime-trigram.bin` 需在下次正式跑 `msime-dict-build` 携带 `--extra-ngram-corpus` 重新发版词库时才会把种子语料反映进二进制表。

## Verification

- `cargo test -p msime-engine --lib`（2741 passed, 0 failed, 57 ignored，含 `nine_key::tests::finish_reuses_the_first_commit_buffer` 恢复 `<= 32` 次分配预算）；清理未用导入后复跑 `quanpin::dictionary::tests::canonical_writer_calibrates_new_word_weight_below_low_frequency_peers`（1 passed）。
- `cargo test -p msime-client-services --lib -- dictionary`（135 passed, 0 failed, 1 ignored）、`cargo test -p msime-host-api --lib -- dictionary`（182 passed, 0 failed, 3 ignored）、`cargo test -p msime-dict-builder`（185 passed, 0 failed, 1 ignored；`tests/web.rs` 11 passed）。
- `python3 scripts/test-dict-builder-private-reader.py`、`cargo fmt --check`、`git diff --check` 与 `pnpm run verify-notes`（2082 notes + 4 archived notes）全部通过。
- 本机 `rustc 1.98.1` 下 `cargo clippy -p msime-engine --all-targets -- -D warnings` 零告警通过；`msime-engine`、`msime-client-services`、`msime-host-api`、`msime-dict-builder` 经 `--no-deps ... -D warnings -A clippy::chunks_exact_to_as_chunks -A clippy::field_reassign_with_default`（排除未改旧文件的 1.98 新 lint 提示）全部零告警通过。
- `bash scripts/verify-local.sh --quick` 通过全部 400+ 契约检查、`agent notes`、`compile: rust workspace`、`compile: linux desktop shell`、`compile: linux native host`（112/112 passed）与 `compile: shared apple bridge`；仅 `compile: macos` 因本机仅有 CommandLineTools（无 `Xcode.app` 的 `SwiftUIMacros` 插件）在未改动的 `BackendEmojiAppearance.swift` 测试目标上失败，产品宿主目标 `MSIMEClientInputMethod`（含 `cmake --build target/macos-isolated --target MSIMEClientInputMethod`）与 `build-shurufa.sh` 构建通过。

## Related

- [超高频核心词渐进调频护城河与语境同音竞争解耦](../bug-fix/2026-11-18-core-word-frequency-moat.md)
- [导入 Rime 百分比权重时保留合法词条](../bug-fix/2026-10-18-rime-percentage-weight.md)
- [Gboard v1 文本词库复用共享导入链路](2026-10-20-gboard-dictionary-import.md)
- [词网格不再因为负的二元分扣分](../bug-fix/2026-10-13-lattice-negative-bigram.md)
