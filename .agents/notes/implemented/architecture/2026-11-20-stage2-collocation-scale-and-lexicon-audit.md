# Agent Note: Stage 2 搭配模型量纲修复与万象词库 2×2 消融审计

Status: implemented

## Problem

在 [Shurufa 候选词决策链五层重构与分阶段演进](../../../proposed/architecture/2026-11-20-shurufa-candidate-pipeline-redesign.md) 的 Stage 2 中，我们需要同时排查并验证两个底层概率底座假设：

1. **量纲 Bug（`LOG10_E` 衰减）**：词网格主分（`crates/engine/src/lattice/decode.rs`）中单字、多字词、二元/三元与个人上下文全部运行在自然对数（`ln` / nat）域，但 `crates/engine/src/quanpin/dictionary.rs` 与 `crates/engine/src/shuangpin/dictionary.rs` 在装配万象字级搭配模型（`octagram::GramDb::query`，同样返回自然对数域）时误以为词网格主分是 `log10` 口径，多乘了一次 `std::f64::consts::LOG10_E`（`≈ 0.4343`），导致万象 `.gram` 的搭配信号在 Viterbi 搜索中被系统性腰斩一半以上。
2. **底词库与解码公式 2×2 消融**：上游提案计划用同源 `rime-wanxiang-base`（`v18.1.14`）词表替换旧 `msime-pinyin.db`（`dict-v2.0.24`），并切到纯概率解码 `ln(w) - ln(Z)`，但三方共识设立了硬验收门：**先单独修复 `LOG10_E` 量纲 Bug，再跑「旧/新词库 × 旧/新解码公式」的 2×2 消融实验；任何替换不得在核心评测集上回退。**

## Decision

严格按 2×2 实测结果决策：**立即落地 `LOG10_E` 量纲修复（大幅全线提分），同时拒绝将主词库切换为裸 `rime-wanxiang-base` 或实验性无二元概率解码，保留现行 `msime-pinyin.db` + `wanxiang-lts-zh-hans.gram` + `msime-bigram/trigram.bin` 组合。**

### 1. 落地改动

- **`crates/engine/src/quanpin/dictionary.rs`、`crates/engine/src/shuangpin/dictionary.rs`、`crates/engine/src/lattice/octagram.rs`、`crates/engine/src/lattice/decode.rs`、`crates/engine/src/lattice/decode_collocation_tests.rs`**：
  - 移除装配 `GramDb::query(tail, word, is_rear, &config)` 时多余的 `* std::f64::consts::LOG10_E`，让 `collocation_weight` 成为纯粹的自然对数域标定系数。
- **`crates/input-runtime/examples/convert_eval/main.rs`**：
  - 增加 `--no-neural` 与 `--collocation-weight <f64>` 选项，供后续 Stage 3/4 与离线冷启动消融复现。

### 2. 实测数据与 2×2 消融记录

#### A. 仅修复 `LOG10_E` 的完整管线收益（万象 `.gram` w=1.0 + 4.5MB 逐键 + 25MB 停顿 + 自动纠错，页宽 6）

| 评测集 | Stage 0 基线（含 `LOG10_E` Bug） | Stage 2A（修复 `LOG10_E` 后） | 变化 |
|---|---|---|---|
| `shurufa-spoken-v1` (n=120) | `imm_top1=0.725` / `top1=0.758` / `top5=0.917` / `found=0.942` / `mrr=0.827` / `flips=6` / `pruned=7` | **`imm_top1=0.783` / `top1=0.792` / `top5=0.958` / `found=0.992` / `mrr=0.864` / `flips=2` / `pruned=1`** | **`imm +5.8%`, `top1 +3.4%`, `found +5.0%`, `flips -67%`** |
| `sentences-v2` (n=312) | `imm_top1=0.663` / `top1=0.740` / `found=0.971` / `mrr=0.837` / `flips=49` | **`imm_top1=0.740` / `top1=0.779` / `found=0.984` / `mrr=0.853` / `flips=30`** | **`imm +7.7%`, `top1 +3.9%`, `found +1.3%`, `flips -39%`** |
| `sentences-neutral-v1` (n=150) | — | **`imm_top1=0.784` / `top1=0.842` / `found=0.978` / `flips=9`** | — |

#### B. 裸万象词表 `rime-wanxiang-base` v18.1.14（`CC-BY-4.0`，SHA-256 `6aeb64b0...`）冷启动 2×2 消融（`--no-neural`，修复 `LOG10_E` 后）

| 配置 | `quanpin-words-v1` (n=25,119) | `sentences-neutral-v1` (n=1,105) | `sentences-v2` (n=312) | `shurufa-spoken-v1` (n=120) |
|---|---|---|---|---|
| `old` + `legacy` | **`0.7450`** | **`0.8130`** (`pruned=23`) | **`0.7790`** | `0.7170` |
| `old` + `prob` | `0.7450` | `0.7950` (`pruned=36`) | `0.7660` | `0.7500` |
| `wanxiang` + `legacy` | `0.7150` (`-3.0%`) | `0.7670` (`-4.6%`, `pruned=56`) | `0.7760` | `0.7500` |
| `wanxiang` + `prob` | `0.7150` (`-3.0%`) | `0.7770` (`-3.6%`, `pruned=49`) | `0.7690` | **`0.7580`** |

审计源码与 SQLite 分布定位出裸 `rime-wanxiang-base` 未过门禁的两处结构性缺陷：

1. **`zi.dict.yaml` 并非真实语料单字词频**：万象人为将每个音节的前 4 个单字整体抬升至 `2.4M–48M`（例如 `在=4.51M, 再=4.44M, 载=3.44M, 灾=2.45M`、`损=7511 > 孙=3069`），严重扭曲单字与词组、同音字之间的相对概率，导致整句大量碎拆成单字（`是+时间+是` 压过 `实时+监视`，`于+被` 压过 `预备`）；而旧 `msime-pinyin.db` 的 `tbl_1_*` 保留了接近真实语料 Zipf 分布的单字频次（`在=19.2M, 再=2.68M, 载=127k, 灾=83k`，`孙=421k > 损=160k`）。
2. **`jichu.dict.yaml` 混入 133 万条高噪 3–4 字词与艺人/地名垃圾**：3 字词条高达 67.8 万且中位权重（364）反超 2 字词（336），叠加 `yiren`/`diming` 使 `韩磊` 压过 `含泪`、`管理码` 压过 `管理+吗`、`不要的+最` 压过 `不要+得罪`。

#### C. 均衡混合词库 `shurufa-v3`（Zipf `tbl_1` + 对数几何均值混合 + 高置信现代口语补词）完整管线对比

| 配置（含 4.5MB + 25MB + `.gram`） | `spoken` (n=120) | `sentences-v2` (n=312) | `neutral-150` |
|---|---|---|---|
| **`old_legacy + LOG10_E`** | **`imm=0.783 / t1=0.792 / fnd=0.992 / flp=2`** | `imm=0.740 / t1=0.779 / fnd=0.984 / flp=30` | **`imm=0.784 / t1=0.842 / fnd=0.978 / flp=9`** |
| `v3_legacy + LOG10_E` | `imm=0.767 / t1=0.792 / fnd=0.983 / flp=3` | `imm=0.734 / t1=0.792 / fnd=0.981 / flp=35` | `imm=0.784 / t1=0.827 / fnd=0.964 / flp=14` |
| `v3_prob(26,18,1.0)`（停用旧 bigram/trigram） | `imm=0.733 / t1=0.750 / fnd=0.933 / flp=6` | `imm=0.718 / t1=0.782 / fnd=0.981 / flp=37` | `imm=0.791 / t1=0.820 / fnd=0.978 / flp=10` |

即便将万象与旧词库做几何均值融合（`v3`），开启神经模型后仍在 `spoken` 即时首选与召回、`neutral` 首选与召回及全集抖动次数上弱于 `old_legacy + LOG10_E`；同时 `v3_prob` 停用 `msime-bigram/trigram.bin` 后，在 `.gram` 未覆盖的口语转折与句首（`.gram` 无 `<bos>` 标记）损失了 5.9% 的 `spoken` 召回。因此不切默认主词库、也不在生产代码保留未激活的概率解码死分支。

## Historical audit

- [Shurufa 候选词决策链五层重构与分阶段演进](../../../proposed/architecture/2026-11-20-shurufa-candidate-pipeline-redesign.md)：本篇落地其中的 Stage 2。原提案预期 Stage 2 用万象词表替换旧底词库并废弃旧二元/三元表；经 2×2 门禁实测否决裸替换，保留 `LOG10_E` 量纲修复作为 Stage 2 正式产出，并将剩余同音/语境与重排痛点精准收敛至 Stage 3（统一单遍打分 + 停顿锁定首选 + 语境同音重排）与 Stage 4（旁路个人学习）。
- [词网格接入 Octagram 语法模型](../feature/2026-10-21-octagram-grammar-model.md) 与 [Shurufa 默认接入万象语法模型](../feature/2026-11-18-shurufa-wanxiang-grammar-model.md)：修补装配层将 `GramDb::query` 自然对数域误乘 `LOG10_E` 的历史遗留缺陷。

## Alternatives considered

### 直接将 Shurufa 主词库替换为 `rime-wanxiang-base` v18.1.14
最强理由是与 `wanxiang-lts-zh-hans.gram` 同源、许可证同为 `CC-BY-4.0` 且 174 万词条 100% 匹配 Shurufa 423 完整音节表。未采用，因为 2×2 消融证实其 `zi.dict.yaml` 人工拉平前 4 字与 133 万高噪 3–4 字词在 `quanpin-words-v1`（`-3.0%`）和 `sentences-neutral-v1`（`-4.6%`）上显著回退，未通过门禁。

### 上线几何均值混合词库 `shurufa-v3` 与 `shurufa_dict_meta` 双轨概率解码
最强理由是保留 Zipf 单字分布的同时补齐部分现代口语词频。未采用，因为完整管线实测仍弱于仅修 `LOG10_E` 的现行词库，且引入第二套词库构建分发与 `editions.py` 锁维护成本，与「删除叠加复杂度而非再造双轨」的架构原则相悖。

## Consequences

- **收益**：改动最小（仅移除两处错误的 `* LOG10_E` 常数并修正文档/单测），全管线零新增延迟、零额外内存、零锁文件风险，即刻释放万象 `.gram` 完整信号：`spoken` 即时首选 `+5.8%`、召回达 `99.2%`，`sentences-v2` 即时首选 `+7.7%`，停顿翻脸率同步下降 39%–67%。
- **后续方向**：Stage 2 消融清晰排除了「底词库词频不够新」作为当前最大瓶颈，下一阶段（Stage 3）将集中消除逐键双跑 4.5MB 模型、锁定停顿首选消除 `flips`、并打通精确词典同音词的语境重排（直指 `pronoun-ta` 与 `context-homophone`）。
