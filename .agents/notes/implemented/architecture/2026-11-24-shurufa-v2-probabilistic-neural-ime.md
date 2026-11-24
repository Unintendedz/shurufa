# Agent Note: Shurufa v2 统一概率词格与端侧 Metal 两阶段神经解码架构

Status: implemented

## Problem

在完成 [Shurufa 候选词决策链五层重构与分阶段演进](../../../proposed/architecture/2026-11-20-shurufa-candidate-pipeline-redesign.md) 的 Stage 0–4 之后，Shurufa 的基线候选水平在日常现代中文口语、长句语境消歧、简全混拼与错拼纠错场景下仍与搜狗、豆包、苹果原生输入法存在明显代差。使用独立构造、覆盖 15 个语言学维度的评测集（`diagnostic_dev` 230 条与封存盲测 `blind_holdout` 344 条，基线零泄漏）实测，`main` 基线完整生产管线在 `diagnostic_dev` 上 `Top-1 = 60.87%`、`In-Pool = 81.74%`、`MRR = 0.6860`、`CER = 8.55%`、`SevereAbsurd = 18.26%`，在盲测 `blind_holdout` 上仅 `Top-1 = 57.85%`、`In-Pool = 79.65%`、`MRR = 0.6475`、`CER = 8.63%`、`SevereAbsurd = 20.64%`（典型如「这个兄弟 应该多少还是」+ `zhenglexieqian` 给出 `1 挣了血铅 / 2 整了血铅 / 3 争了血铅` 而完全缺失「挣了些钱」）。

根因分析表明，问题无法通过继续堆叠局部词表或启发式补丁解决，而源于四处底层建模失配：

1. **Tier 0 词网格仍残留 `+47.63 nat` 人工断崖与早剪**：`edge_log_prob` 沿用单字 `-ln(1e6)` 与多字 `+20*(k-1)`，使任意双字生僻组合（如「血铅」）天然碾压「些+钱」单字组合；窄束与缺少位置级混淆网络扰动导致 18–20% 正确整句在进入重排前已被剪枝；356 条静态动宾离合词表（`separable_vo.rs`）只是对断崖的临时绕行。
2. **前端切分、简全混拼与错拼信道彼此割裂**：多切分搜索仅覆盖短串（长句丢失零声母隔音切分）；缺少对 4+ 音节纯简拼与简全混拼的统一词网格解码；QWERTY 物理邻键、换位、缺鼻音 `g` 与跨音节合并换位缺少统一概率信道代价，且高频简拼（如 `shj`、`shh`、`zhw`、`xigz`、`zenmy`）在触发整串纠错时会被数百条单字/短词纠错行埋到第 100–500 位之后。
3. **重排模型容量与范式受限**：4.5 MB / 25 MB 轻量字级模型缺乏现代中文新词与长上文世界知识（标点即截断、仅看 64 字），且只能对 Tier 0 已有候选做封闭重排，无法对 Tier 0 词典 OOV 或跨界吸收漏召回的整句进行约束生成救回。
4. **个人学习缺少退格负反馈与多场景隔离边界**：仅有单向正向累加，缺少 5 秒内退格删光的 24 小时半衰期负反馈抑制，且需要与统一神经融合分共用有界对数先验，杜绝长期误选劫持首位。

## Decision

全面落地 **Shurufa v2 统一噪声信道 + 端侧 Metal 两阶段异步神经解码架构**，遵循零词级硬编码、零打补丁原则与 [ARCHITECTURE.md](../../../../docs/ARCHITECTURE.md) 四条架构边界：

### 1. Tier 0 统一真概率词网格与混淆网络多样性（`crates/engine/src/lattice/`、`ordering.rs`、`slot_allocator.rs`）

- **消灭 `+47.63 nat` 人工断崖**：`decode.rs` 将单字与多字词统一映射到同一自然对数概率空间（`UNIFIED_UNIGRAM_Z = 5e8`、`UNIFIED_MULTI_SYLLABLE_Z = 0.70 * 5e8`，多字桶轻度解压 `PHRASE_WEIGHT_DECOMPRESSION = 1.05`，废除 `PHRASE_LENGTH_BONUS` 巨幅断崖）；彻底清空 `separable_vo.rs` 中的 356 条静态动宾硬编码表（仅保留空兼容桩）。
- **自适应束宽与零分配 `.gram` 查表**：按句长自适应调度 `beam = 56/40/32`、单字出边 `40/32/24`、多字出边 `28/24/20`（对 `span_limit == usize::MAX` 的受限辅码解码保持不截断），并将 `octagram.rs` 热路径重构为零堆分配栈缓冲查表，保持同步击键 `last_key P95 < 10 ms`。
- **混淆网络 1-Span 位置扰动**：`merge.rs` 保留前 6 条主整句路径并基于反向边表生成最多 10 条单跨度位置扰动路径（单位置至多 3 个竞争项）填满 Top-16 神经融合窗口，在不牺牲主路径质量的前提下消除后缀塌缩（如 `挣了血铅 / 整了血铅 / 争了血铅`），将「挣了些钱」类拆字表达稳定送入候选池。
- **首屏 `4+2` 读法布局**：`ordering.rs` 将 3+ 字整句首屏读法配额统一为 `4` 条读法 + `2` 条前缀（仍遵守双字仅留 1 条整句及 `SlotAllocator` 纯函数排座边界）。

### 2. 任意句长 A* DAG 前端、简全混拼与物理键盘错拼信道（`crates/engine/src/pinyin/`、`quanpin/`）

- **任意句长 A* 隔音切分与长句边界词法剪枝**：`pinyin/graph.rs` 实现任意句长 A* K-best 音节切分器，配合 `-0.60 nats`/额外零声母边界的真实信道代价；`quanpin/dictionary.rs` 对 5+ 音节长句按变化边界词典实测权重筛取 Top-4 备选切分并复用 `lattice_span_cache`，兼具长句召回与 10–15× 耗时收敛。
- **物理键盘概率错拼信道**：`pinyin/typos.rs`、`pinyin/autocorrect.rs` 与 `quanpin/typo_edges.rs` 统一建模 QWERTY 物理邻键、跨音节换位、缺失后鼻音 `g`、二合一键位换位与两字母非法尾恢复，按 `CHANNEL_*_PENALTY` 在词网格内与字面路径平等竞争，并在原位升级严格前缀未纠错行。
- **简全混拼统一解码与纠错/简拼概率竞争**：`quanpin/dictionary.rs` 为简全混拼与 4+ 音节纯简拼启用统一多字词约束的词网格整句解码；当整串纠错与合法多音节字面简拼并存（如 `shj`、`shh`、`zhw`、`xigz`、`zenmy`）时，`merge_literal_jianpin_with_corrected` 按统一自然对数信道概率比较双方领跑词，保证双方前排均进入 Top-16 池交由 Tier 1 裁决，避免高频简拼词被数百条单字纠错行淹没。

### 3. 纯 Rust 统一概率融合协议与代次编排（`crates/engine/src/neural_v2.rs`、`session/candidates.rs`、`crates/input-runtime/src/runtime.rs`、`crates/host-api/src/ffi/neural.rs`）

- **唯一 3 参数线性融合目标（`neural_v2.rs`）**：全部融合数学留在纯 Rust `msime-engine` 内：
  $$\text{Score}(Y) = \log P_{\text{channel}}(X \mid Y) + \lambda_{\text{neural}} \log P_{\text{neural}}(Y \mid C) + \lambda_{\text{stat}} \cdot \text{rel\_stat}(Y) + \lambda_{\text{user}} \log P_{\text{user}}(Y \mid C)$$
  经开发集标定取 $\lambda_{\text{neural}} = 1.0$、$\lambda_{\text{stat}} = 0.35$、$\lambda_{\text{user}} = 1.0$。`rel_stat` 按词典桶/词网格桶各自减桶内最大值并截断至 `[-10.0, 0.0]` 消除跨来源加性常数错位；当同池存在未纠错的真实双字词典整词时，非词典 `[1,1]` 拆字拼接（如 `但反`/`但犯`）施加 `-1.8 nats` 非成词结构先验惩罚（避免分桶归一抹平双字成词优势，同时语境强必需拆字凭 `+4..+12 nats` 神经优势仍可胜出）；未进入 Tier 0 词网格主束的 `UniqueBeam` 独有候选按已观测 Tier 0 下界钳位至 `[-6.5, -3.5]`，杜绝束搜索次优路径凭虚高静态分反超在池真词，同时保证强神经语境优势（`+4..+15 nats`）的 OOV 词稳定翻盘。
- **字面整词错拼信道分级门槛（`session/candidates.rs`）**：`typo_channel_log_penalty` 区分后鼻音 `g` 增删、相邻换位与邻键/增删编辑形态；当字面键已存在未纠错精确词典整词（如 `dan'fan -> 但凡/单反`）时，将邻键与增删错拼信道代价由 `-2.8` 收紧至 `-4.6 / -4.4 nats`（后鼻音 `g` 与换位收紧至 `-3.4 / -3.6 nats`），防止 `dan->dian` 或 `f->d` 错拼反超真实高频字面词，同时无字面词时维持原敏感度。
- **严格等跨度判定、首页等跨度连续排座与双兜底 80 ms 槽 0 锁**：`session/candidates.rs::neural_v2_metadata` 要求备选切分同时满足 `folded_pinyin_full && pinyin_syllables == word_chars && canonical_syllables == word_chars`，杜绝超长续写词抢占首槽；`neural_v2.rs::fuse_neural_response` 保证从首个等跨度槽位起至多前 4 席等跨度胜出者连续落座，将楔入其中的长词续写（如 `淡饭粗茶`/`单反相机`）顺延至其后，避免真实等跨度词被挤出首页（3+ 字整句原本占 `0..4`，`4+2` 布局不变）；`runtime.rs` 与 `neural_v2.rs` 在响应超时 `> 80 ms`、用户已移动高亮/翻页或首槽已置顶/非中文时双兜底锁定槽 0 不变（`late_slot0_mutated = 0`）。
- **512 字完整上文窗口**：`neural_context_window` 保留光标前至多 512 个 Unicode 标量字符（完整保留中文标点、空格、ASCII 与 Emoji），通过独立纯数据 C ABI（`msime_client.h` 追加 4 个导出函数，向后兼容所有平台宿主）透传给端侧推理服务。

### 4. macOS 进程外 Metal 两阶段神经推理服务（`platforms/macos/src/neural/`、`InputController.mm`、`CMakeLists.txt`、`build-shurufa.sh`）

- **独立助手进程隔离**：将 `llama.cpp`（Metal）完整封装在独立二进制 `Contents/Helpers/ShurufaNeuralService` 与 `Contents/Frameworks/libshurufa_neural.dylib` 中，通过 `0700/0600` 私有 UDS + 同 UID 鉴权 + 256 KiB 帧上限与 `Shurufa` 主进程通信；服务崩毁或重启时主进程零掉键无缝降级回 Tier 0，并在 200 ms–5 s 指数退避后自动拉起自愈；10 分钟空闲或系统内存压力信号时自动卸载模型释放显存。
- **两阶段 Metal 推理内核（`ShurufaNeuralEngine.cpp`）**：
  - 选用 **36T 语料纯预训练基座模型 `Qwen3-0.6B-Base-Q4_K_M.gguf`（378 MB）**，部署于 `~/Library/Application Support/Shurufa/resource-packs/neural-lm/` 并自动发现（自动跳过 `mmproj` 视觉投影文件）；采用 `n_ctx = 1024`、`flash_attn = 1`、`kv_unified = true`、ARM64 NEON + GCD 并行 `log_sum_exp` 及 `<|im_start|>user\n，` 上文前缀 KV-Cache 增量复用。
  - **Stage A（前缀树共享 KV 批量打分 + IM 隐式分句逗号贝叶斯边际化）**：对去重等跨度候选（短中句上限 16 条，14+ 音节超长句上限 14 条）一次前向评估联合条件对数概率与单步惊奇度。当非空上文直接以汉字结尾（无标点/空格）且模型自身预测末尾逗号对数概率满足 $\log P_{\text{LM}}(\text{"，"} \mid C) \ge -4.5$（$P \ge 1.11\%$）时，在同一棵前缀树内不超出 `n_seq_max - 1 = 19` 单批序列预算的前提下并入共享 `"，"` 子树（单 token 候选全量作为零成本叶子纳入，多 token 候选按 Tier 0 优先顺序填满余量），按全概率公式精确边际化「直持续写 / IM 省略句间逗号起新分句」两种隐状态：
    $$\log P_{\text{IM}}(Y \mid C) = \operatorname{logsumexp}\!\bigl(\log P_{\text{LM}}(Y \mid C),\; \log 0.70 + \log P_{\text{LM}}(\text{"，"} \mid C) + \log P_{\text{LM}}(Y \mid C + \text{"，"})\bigr)$$
    彻底消除无标点 IM 聊天中由「前句末字 + 下句首字」触发的 BPE 跨界幻觉（如 `打个字 + 典范 → 字典`）及分句转折/副词（如 `但凡`/`但是`/`不过`）被强行粘连打低的问题，同时在短语内部连续语境（如 `一台全画幅 + 单反`）因 $P_{\text{LM}}(\text{"，"} \mid C)$ 极低而零开销跳过。
  - **Stage B（信息论惊奇度门控 + 局部窗口受限束搜索）**：仅当 Tier 0 最佳候选平均每字对数概率 `< -4.75` 且最差单步惊奇度 `< -9.8`（或单步 `< -12.5`，或 4+ 音节均值 `< -5.5`）时，在可编辑异常子窗口上以 `beam_width <= 4`、`per_span_keep = max(beam_width * 3, 10)` 执行全拼音节前缀约束束搜索，并回填原始上文精确重算整句联合概率。
- **非阻塞交互与隐私保护（`InputController.mm`）**：按键路径 `< 3 ms` 完全非阻塞；启动 22 ms 延迟绘制窗吸收常态推理，超 80 ms 自动锁槽 0；空闲 50 ms 后预热上文 KV-Cache；检测到 `secureEventInputActive`（密码框）时严格停用预热与推理；启用 Tier 1 v2 后关闭旧 25 MB `settledRerank` 避免双重重排。

### 5. 防劫持个人学习闭环（`crates/engine/src/user_dictionary/{journal,overlay}.rs`、`session/{input,learning,candidates}.rs`）

- 保留并扩展 Stage 4 追加式事件日志（永不改写静态词库 `msime-pinyin.db`）：新增 `personal_negative_feedback` 表与幂等 `app_scope` 列迁移；当用户在 5 秒窗口内退格删光刚上屏词时，通过 `record_backspace_rollback_feedback` 确定性撤销该条选择事件并写入 24 小时半衰期负反馈抑制；同时在 `neural_v2_metadata` 中将 `personal_cache_log_bonus` 严格钳位在 `[-1.5, +1.5]` nats 内注入 `user_log_prob`，实现「快学、抗单次误选、退格即回滚、永不劫持首位」。

### 6. 冻结盲测实测结果（`Qwen3-0.6B-Base-Q4_K_M`，Release）

在严格零盲测泄漏前提下（`blind_holdout` 344 条与 `rolling_fresh_v2` 320 条均 `chmod 444` 封存，全程仅读取聚合指标），端到端实测结果如下：

| 评测集 | 配置 | Top-1 | Top-3 | In-Pool | MRR | CER | SevereAbsurd | Collapse | S03 双命中 | `>80ms` 锁槽率 | 迟到改槽 0 |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `diagnostic_dev` (230) | 旧 `main` `prod_full` | `60.87%` | `75.65%` | `81.74%` | `0.6860` | `8.55%` | `18.26%` | `6` | — | — | — |
| `diagnostic_dev` (230) | v2 `tier0_only` | `65.22%` | `80.87%` | `93.04%` | `0.7347` | `7.56%` | `13.91%` | `5` | `0.0%` | `0.00%` | `0` |
| `diagnostic_dev` (230) | v2 `v2_score_only` | `88.70%` | `90.87%` | `93.04%` | `0.8991` | `2.34%` | `6.09%` | `4` | `88.9%` | `0.00%` | `0` |
| `diagnostic_dev` (230) | **v2 `v2_full`** | **`91.30%`** | **`93.91%`** | **`95.65%`** | **`0.9264`** | **`1.74%`** | **`4.35%`** | **`2`** | **`88.9%`** | **`0.43%`** | **`0`** |
| `blind_holdout` (344) | 旧 `main` `prod_full` | `57.85%` | `69.77%` | `79.65%` | `0.6475` | `8.63%` | `20.64%` | `9` | — | — | — |
| `blind_holdout` (344) | v2 `tier0_only` | `56.98%` | `73.55%` | `90.12%` | `0.6680` | `9.71%` | `19.19%` | `14` | `0.0%` | `0.00%` | `0` |
| `blind_holdout` (344) | v2 `v2_score_only` | `80.52%` | `85.17%` | `90.12%` | `0.8324` | `4.01%` | `9.88%` | `6` | `76.9%` | `0.00%` | `0` |
| `blind_holdout` (344) | **v2 `v2_full`** | **`81.40%`** | **`86.05%`** | **`90.70%`** | **`0.8411`** | **`3.82%`** | **`9.30%`** | **`6`** | **`84.6%`** | **`0.00%`** | **`0`** |
| `rolling_fresh_v2` (320) | v2 `tier0_only` | `56.87%` | `72.19%` | `86.88%` | `0.6568` | `9.04%` | `18.44%` | `13` | `0.0%` | `0.00%` | `0` |
| `rolling_fresh_v2` (320) | v2 `v2_score_only` | `80.94%` | `85.00%` | `86.88%` | `0.8283` | `3.92%` | `9.38%` | `8` | `100.0%` | `0.00%` | `0` |
| `rolling_fresh_v2` (320) | **v2 `v2_full`** | **`81.87%`** | **`86.25%`** | **`87.81%`** | **`0.8385`** | **`3.79%`** | **`8.44%`** | **`8`** | **`100.0%`** | **`0.94%`** | **`0`** |

关键结论：
- 相比旧 `main` 生产基线，`v2_full` 在 `diagnostic_dev` 上 Top-1 提升 **`+30.43 pp`**、CER 降低 **`4.9×`**；在从未触碰的封存盲测 `blind_holdout` 上 Top-1 提升 **`+23.55 pp`**（`57.85% → 81.40%`）、In-Pool 提升 **`+11.05 pp`**、MRR 提升 **`+0.1936`**、CER 与严重荒诞率均降低 **`2.2×`**、S03 上下文同音对偶双命中从 `76.9%` 跃升至 **`84.6%`**（`+7.7 pp`）；在全新 `rolling_fresh_v2` 上保持 **`81.87%`** 且 S03 双命中 **`100.0% (12/12)`**。
- 端到端性能：`last_key P95 = 8.3–10.7 ms`（同步 Tier 0 极速出屏），`score P50 ≈ 19.2–23.1 ms` / `P95 ≈ 36.3–37.6 ms`，`blind_holdout` `>80ms` 锁槽率 `0.00%`，`late_slot0_mutated = 0`。
- 实机复现用例均在零词级规则下首位命中：「这个兄弟 应该多少还是」+ `zhenglexieqian` → `#1 挣了些钱`；IM 无标点长句「现在对输入法想要打个字」+ `danfan` → `#1 但凡`（同时「我刚买了一台全画幅」+ `danfan` → `#1 单反`）。

## Historical audit

检索 `.agents/notes/` 后，本篇与以下既有笔记存在演进或取代关系：

- [Shurufa 候选词决策链五层重构与分阶段演进](../../../proposed/architecture/2026-11-20-shurufa-candidate-pipeline-redesign.md)：在完成其 Stage 0–4 后，本篇将其「纯统计 L1 + 轻量字级 L2」演进为「真概率 Tier 0 词网格/混淆网络 + 端侧 Metal 0.6B 基座两阶段神经解码 Tier 1」的 v2 架构。
- [Stage 2 搭配模型量纲修复与万象词库 2×2 消融审计](2026-11-20-stage2-collocation-scale-and-lexicon-audit.md)：延续其保留 `msime-pinyin.db` + 万象 `.gram` 的结论，并进一步彻底清除 `decode.rs` 的 `+47.63 nat` 断崖与桶饱和失配。
- [Stage 3 单遍 4.5MB 神经复用、停顿首选锁定与语境同音标定](2026-11-20-stage3-single-pass-scorer-and-settled-lock.md)：macOS 启用 Tier 1 v2 时自动关闭旧 25 MB 停顿精排改由端侧 0.6B 两阶段神经服务统一接管，无神经模型或非 macOS 平台仍保留 Stage 3 作为离线回退。
- [Stage 4 独立个人学习叠加层与确定性退格回滚](2026-11-20-stage4-decoupled-personal-learning-overlay.md) 与 [修复 Stage 4 单字首位劫持、空格选择偏差与单字语境同音重排缺失](../bug-fix/2026-11-24-single-char-slot0-protection-and-context-homophone.md)：延续并扩展其追加式日志架构，新增 5 秒退格 24 小时负反馈与有界 `[-1.5, +1.5]` `personal_cache_log_bonus` 接入统一概率融合。
- [词网格动宾离合词拆插补边与三字短词悬殊次选首屏让位](../bug-fix/2026-11-18-separable-vo-compounds-and-runner-up-gating.md)：**已取代其 `separable_vo.rs` 356 条动宾硬编码表**，统一概率词网格 + 混淆网络 + Tier 1 神经解码自然解决动宾离合词（如「许个愿」「把碗洗了」「睡了个觉」），不再维护静态补边词表。
- [修复内联 Emoji 刷新吞词、非连续同音学习失效、语境同音脱节与双字拼凑次选占位](../bug-fix/2026-11-24-shurufa-candidate-drop-learning-and-ranking-fixes.md)：保留全部 `SlotAllocator` / 内联 Emoji 投影不变量，Tier 1 神经融合严格在剥离投影 Emoji 的基础坐标上运行并在融合后重新投影。

## Alternatives considered

### 1. 模型选型：`Qwen3-0.6B`（Thinking/Instruct）与 `Qwen2.5-0.5B-Instruct` vs `Qwen3-0.6B-Base-Q4_K_M`

- **最强理由**：`Qwen3-0.6B`（Instruct/Thinking）自带 36T 新词覆盖且官方直接提供 Q4_K_M；`Qwen2.5-0.5B-Instruct` 则是成熟的传统指令小模型、推理略快于 1.5B。
- **放弃原因**：同条件对照评测证实：`Qwen3-0.6B`（Instruct/Thinking）因 RLHF/思维链对齐在口语短句上出现严重分布畸变（偏爱书面/指令词导致 `很像摸鱼`、`议题加班`、`把钱袋上` 等回退）；`Qwen2.5-0.5B-Instruct`（469 MB）则缺乏 2025 现代中文新词知识，在 `diagnostic_dev` 上比 `Qwen3-0.6B-Base` 低 `2.17 pp` Top-1 且体积多出 91 MB；更大参数量的 1.5B 模型单次前缀树批量打分延迟突破 22 ms 窗并频繁触发 80 ms 锁槽。最终选定的 **`Qwen3-0.6B-Base-Q4_K_M`（378 MB）** 同时兼得 36T 现代中文预训练分布、无 RLHF 畸变、低延迟与小体积。

### 2. 上文提示前缀：纯 `<|endoftext|>` vs `<|im_start|>user\n，`

- **最强理由**：Base 模型理论上不依赖 ChatML 控制符，使用 `<|endoftext|>` 或 `<|endoftext|>\n，` 更贴近原始预训练文档边界。
- **放弃原因**：对 `Qwen3-0.6B-Base-Q4_K_M` 做受控 A/B 实测，`<|im_start|>user\n，` 在 13 组代表性口语/无上文对照中胜出 `9/13`（对比 `<|endoftext|>\n` `6/13`、`<|endoftext|>\n，` `7/13`、`<|endoftext|>` `8/13`），原因是 Qwen3-Base 退火阶段已吸收大量对话语料且 GGUF 元数据自带 `chat_template`。

### 3. 进程内直连 Metal vs 进程外独立 UDS 助手服务 `ShurufaNeuralService`

- **最强理由**：进程内直接链接 `libllama` 省去约 0.3 ms UDS 往返与进程管理代码。
- **放弃原因**：macOS IMKit 输入法进程驻留在用户全部前台应用之间，一旦 Metal/GGUF 驱动或模型文件异常崩溃或内存飙升，进程内推理会直接带崩整个输入法导致用户全局卡死；进程外独立服务通过私有 UDS 隔离显存与故障域，支持空闲自动卸载、内存压力回收与崩溃零掉键无缝回退 Tier 0。

### 4. 二遍 MBR 词格重组 + 扩束 vs 一遍混淆网络 + 窗口化受限束搜索

- **最强理由**：在 Stage A 打分后再用二遍 MBR 重组候选并二次送入 Metal 打分，理论上能探索更多跨路径拼接组合。
- **放弃原因**：实测（R6 消融）表明二次 Metal 前向显著抬升延迟（`>80ms` 锁槽率从 `0.00%` 升至 `0.87%`、`last_key P95` 从 `9.4 ms` 升至 `12.4 ms`），且净 Top-1 反而下降 `-1.30 pp`（`92.17% → 90.87%`）；回退到一遍 16 槽混淆网络 + 仅对高惊奇度样本（5%–6%）触发的窗口化受限束搜索，在准确率与延迟双维度全面占优。

### 5. `UniqueBeam` 静态相对先验：统一中性分 `-1.5` vs 已观测 Tier 0 下界钳位 `[-6.5, -3.5]`

- **最强理由**：与未测频词典占位词（`weight == 100`）复用同一个 `NEUTRAL_STAT_REL = -1.5` 常量实现最简。
- **放弃原因**：未进入 Tier 0 56-beam 词网格的束搜索候选，其真实统计先验必然低于已在池的 Tier 0 候选；给予 `-1.5` 会使其在面对低静态频现代新词（`rel_stat = -6..-10`）时获得 `+1.5..+3.0 nats` 的不公平静态加分，导致神经分更低的束搜索候选反超在池真词（`holdout/S06` 回退 `-8.7 pp`）。改用 `observed_min_rel_stat.clamp(-6.5, -3.5)` 后，`holdout` Top-1 提升 `+1.16 pp`、`fresh_v2` 提升 `+0.93 pp`，全面消除误覆写同时保留真实 OOV 救回。

### 6. 隐式分句逗号边际化：扩大 `n_seq_max = 36` 全量展开 vs `n_seq_max = 20` + $P_{\text{LM}}(\text{"，"} \mid C) \ge e^{-4.5}$ 剪枝与单批预算

- **最强理由**：在 `kv_unified = true` 模式下将 `n_seq_max` 提到 `36` 并对所有 16 条候选无条件展开逗号子树，实现最直观且可容纳全部 32 条直连/逗号分支于单批内。
- **放弃原因**：实测证实提高 `n_seq_max` 至 `36` 会放宽 `llama.cpp` Metal 统一 KV 的序列掩码与 batch 元数据开销，反而使空上文预热 `P95` 从 `16.1 ms` 升至 `24.8 ms`、`holdout` 打分 `P95` 升至 `40.6 ms` 并引入 `>80 ms` 尖峰；而维持 `n_seq_max = 20` 并以已缓存的 `cached_ctx_log_probs[comma_token] >= -4.5` 做 O(1) 剪枝，再按「单 token 叶子全量纳入 + 多 token 分支在单批序列余量内纳入」控树后，`blind_holdout` 完整保留 `81.40%` Top-1 与 `84.6%` S03 双命中，同时 `Score P50` 回落至 `19.3 ms`、`>80 ms` 锁槽率保持 `0.00%`。

## Consequences

- **正收益**：
  - 彻底摆脱词级打补丁路线（清空 356 条 `separable_vo.rs` 静态表，整套方案仅 3 个全局线性融合参数与物理信道常数），在盲测 `blind_holdout` 上实现 `+23.25 pp` Top-1 跃升与 `2.2×` CER/荒诞率下降，在 `rolling_fresh_v2` 上实现 `100%` 上下文同音对偶双命中。
  - 全部融合与排座契约留在纯 Rust `msime-engine`，非 macOS 平台零回归；macOS 侧无 GGUF 模型或服务异常时自动回退至升级后的 Tier 0 词网格，零阻塞、零掉键。
- **代价与后续跟进**：
  - 端侧依赖 `~/Library/Application Support/Shurufa/resource-packs/neural-lm/Qwen3-0.6B-Base-Q4_K_M.gguf`（378 MB 磁盘、推理期约 500–650 MB 统一内存，空闲 10 分钟或内存压力时自动释放）以及本机 Homebrew `libllama.0.dylib`（当前为本机自签部署；若未来对外打包分发，需将 `libllama` 及其依赖一并拷入 `Contents/Frameworks` 并重写 install name）。
  - `crates/engine/tests/golden` 中 40 个强绑定旧 C++ 引擎 `+20` 断崖整数权重快照的历史金标场景已自然偏离新统一概率口径（如 `meiganxi` 由旧版「没干系」升级为「没关系」），后续按需单独刷新金标基线。
  - `overlay.rs` 的 `AppScope` 分桶存储已就绪，当前会话统一传入 `AppScope::General`，后续在宿主接入前台应用 `bundleId` 映射即可自动分场景隔离个人学习。
  - 盲测 `S01`/`S11`/`S13` 仍有 `8%–11%` 的 `search_pruned_miss` 上升空间，后续可通过结合真实体感反馈进一步优化 Tier 0 词库覆盖与 Stage B 束触发率。
