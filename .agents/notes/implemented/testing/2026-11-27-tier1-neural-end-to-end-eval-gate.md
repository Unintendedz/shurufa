# Agent Note: 端侧两阶段（Tier 0 + Tier 1 Qwen3-0.6B）真实模型自动化评测门禁

Status: implemented

## Problem

此前仓库内的自动化评测（`convert_eval`）与本地门禁（`scripts/verify-local.sh`）仅覆盖 Tier 0 统计词格或微型合成 mock 服务（`ShurufaNeuralService --test-mock`），完全没有任何一道本地门禁真正运行「锁定词库 `msime-pinyin.db` + 万象 `wanxiang-lts-zh-hans.gram` + 真实端侧 Metal `ShurufaNeuralService`（`Qwen3-0.6B-Base-Q4_K_M.gguf`）」的完整两阶段链路。

这一验证真空直接导致日常打字成为事实上的 Tier 1 集成测试：
1. **BPE 跨边界伪影无感知**：Qwen3 词表内 `78` 条空格合并汉字（`" 和"`/`" 的"`/`" 是"`/`" 在"`）与中文逗号合并汉字在左边界缺失 Token Healing 时静默掉出首页，Tier 0 单测对此完全盲视；
2. **脏词库与融合标尺回归无兜底**：`weight == 100` 未测频占位整词被挤出 16 候选池、`weight <= 1` 单字幻影异读借上下文反超现代读音，均只有耦合真实 `.gram` 与 `Qwen3-0.6B` 时才会显现；
3. **纠错精度与真错字召回缺乏联合标尺**：干净 4 字句在生产配置（`autocorrect = true, mixed_english = true`）下存在高达 `26.1%`（`24/92`）的 Tier 0 首槽误纠劫持，但若无真实错拼控制组同步守护，盲目收紧纠错罚分又会误伤 `diannai -> 电脑`、`jintina -> 今天`、`meiganxi -> 没关系` 等真错字召回；
4. **展开候选池状态覆写无检测**：`Runtime::all_candidates()` 触发 `expand_cached_candidates()` 覆写已结算 Tier 1 排序的缺陷在纯 Tier 0 评测中无法暴露。

## Decision

在 `crates/input-runtime`、`resources/eval/` 与 `scripts/` 建立 15 秒级端到端真实神经模型门禁体系：

1. **新增统一两阶段评测器 `crates/input-runtime/examples/tier1_eval.rs`**：
   - 复用 [`eval_resources::verify`](2026-10-19-eval-resource-integrity.md) 严格校验锁定词库 SHA-256 指纹，在完全隔离的临时状态目录（`learning = false`，零接触任何真实用户词库或隐私数据）挂载生产一致的 `Session` + `Runtime`（`page_size = 6`、`autocorrect = true`、`mixed_english = true`、`collocation_rerank = true`）；
   - 单次热启动真实 `ShurufaNeuralService` 子进程并完成一次预热解码后，连续评测多套 `--suite <name>=<path.tsv>[:max_syl=N]`，同时采集每条样本的：
     - Tier 0 与 Tier 1 的 `Top1` / `Top6` / `MRR`；
     - 干净拼音首槽被纠错词劫持计数与比例（`t0_typo_hijack` / `t1_typo_hijack`）；
     - Tier 1 相对 Tier 0 的净救回数（`t1_rescues`）与倒退数（`t1_regressions`）；
     - 违禁幻影/占位词抢首或越过金标准计数（`forbidden_hits`）；
     - `Runtime::all_candidates()` 展开操作冲刷已结算首页排序计数（`expand_wiped`）；
     - 热推理端到端时延分位（`p50` / `p90` / `max`）。
   - 支持 `--gate` 硬门禁断言，任一指标跌破红线即输出失败明细并以非零状态码退出。

2. **新增 205 例全合成结构性护栏集 `resources/eval/shurufa-tier1-guardrails-v1.tsv` 与契约检查 `scripts/test-tier1-eval-suite.py`**：
   - 固定覆盖 5 个正交结构切片（全部为 100% 合成用例，无任何真实用户输入）：
     - `boundary-single`（`144` 例）：`24` 个核心高频单字 × `6` 类左边界（空上文、英文+空格 `turnitin `、英文紧贴 `turnitin`、中文逗号 `我觉得，`、换行 `第一行\n`、中文从句）；
     - `phantom-reading`（`14` 例）：`me`/`ne`/`ti`/`wei`/`shui`/`fu`/`gan`/`tai`/`e` 在强诱导语境下的古音/零权幻影防篡位护栏（配 `forbidden_ahead` 断言）；
     - `unmeasured-dict`（`15` 例）：`weight == 100` 日常高频占位整词与 `先哲之石` 对照护栏；
     - `typo-genuine`（`20` 例）：覆盖合法音节邻键/漏 `g`、跨音节 2 合 1 换位（`jintina`/`xihuna`/`linaxi`）与非法音节换位（`sahnghai`/`womne`/`shenem`）的真错字召回控制组；
     - `mixed-english-ctx`（`12` 例）：中英混排上文后接常用双字/三字词稳定性护栏。
   - `scripts/test-tier1-eval-suite.py` 由 `scripts/run-checks.sh` 自动发现执行，静态守卫 TSV 列契约、唯一 ID、5 类切片最低样本量及 `scripts/verify-local.sh` 接线。

3. **新增一键执行入口 `scripts/eval-tier1.sh` 并接入 `scripts/verify-local.sh`**：
   - 自动发现已暂存 `EngineResources`、万象 `.gram`、`ShurufaNeuralService` 与本机 `neural-lm/*.gguf` 模型，一次串联 `spoken-v1`（120 例）+ `guardrails-v1`（205 例）+ `sentences-v2-4char`（92 例）共 `417` 例端到端评测并写出 `target/tier1-eval-latest.json`；
   - 完整 `scripts/verify-local.sh`（及 `verify-local.sh --quick --tier1`）默认执行该门禁（无本地 GGUF 模型环境若未指定 `--require` 则显式打印 `skipped:`）；同时在 `verify-local.sh` 自动将超长 `TMPDIR` 收敛至 `/private/tmp`，根除长临时路径触发 UDS `SUN_LEN` 上限误报。

## Historical audit

- [性能与质量评测先验证真实资源身份](2026-10-19-eval-resource-integrity.md)：直接复用其 `eval_resources::verify` 校验锁文件指纹，不提供跳过资源校验的后门。
- [Shurufa v2 统一概率词格与端侧 Metal 两阶段神经解码架构](../architecture/2026-11-24-shurufa-v2-probabilistic-neural-ime.md)：为其两阶段架构补齐长期缺失的自动化端到端真模型回归门禁。
- [未测频占位整词词网格升格、幻影异读单字护栏与非汉字左边界 BPE Token Healing](../bug-fix/2026-11-27-deferred-unmeasured-placeholder-lattice-promotion.md)：将该次实机爆出的三类缺陷全部固化为 `guardrails-v1` 的永久门禁断言。

## Alternatives considered

### 1. 用 Python `ctypes` 加载 `libmsime_host_api.dylib` 编写外部评测脚本

- **最强理由**：脚本无需 Cargo 编译即可临时修改调试。
- **放弃原因**：macOS 上动态库重签与 `LINKEDIT` 对齐易受 SDK 环境干扰，且经 JSON 字符串轮询 C ABI 无法零损耗读取 `Runtime` 内部候选池与 `take_neural_request()` 入池命中状态；原生 Rust 例程直接链接 `msime-input-runtime` 与 `NeuralServiceClient`，类型安全且 417 例全量实测仅需 `14.1 s`。

### 2. 仅统计干净句准确率，不单列 `typo-genuine` 真错字控制组

- **最强理由**：评测集无需混入带错拼标签的样本，指标计算更简单。
- **放弃原因**：没有真错字召回控制组时，只要简单关掉或大幅重罚纠错信道就能把干净句误纠劫持率刷到 `0%`，却会静默摧毁真实打字纠错体验；同步度量干净句 `typo_hijack_rate` 与 `typo-genuine` 召回率才能守住精确率-召回率帕累托前沿。

## Consequences

- **端到端实测基线（Apple Silicon Metal，`Qwen3-0.6B-Base-Q4_K_M.gguf`，总耗时 `14.05 s`）**：
  - `spoken-v1`（120 例）：Tier 0 `Top1 = 0.750 / Top6 = 0.917` → Tier 1 **`Top1 = 0.875 (+12.5 pp) / Top6 = 0.942`**，净救回 `+15` 例、倒退 `0` 例，`p50 = 21.6 ms` / `p90 = 30.1 ms`；
  - `guardrails-v1`（205 例）：`boundary-single` 144 例 **`Top6 = 1.000 (144/144)`**，`phantom-reading` 14 例 **`Top1 = 1.000`**，`unmeasured-dict` 15 例 **`Top6 = 1.000`**，`typo-genuine` 20 例 `Top6 = 0.850`，违禁篡位 `0` 例，`p50 = 11.0 ms` / `p90 = 23.5 ms`；同时精准暴露 `expand_wiped = 2` 处展开覆写待后续支柱修复；
  - `sentences-v2-4char`（92 例）：Tier 0 `Top1 = 0.522`、首槽误纠劫持 `24/92 (26.1%)` → Tier 1 **`Top1 = 0.891 (+36.9 pp) / Top6 = 0.957`**、净救回 `+34` 例、倒退 `0` 例，量化锁定了下一阶段治理 Tier 0 误纠劫持的精确靶标。
