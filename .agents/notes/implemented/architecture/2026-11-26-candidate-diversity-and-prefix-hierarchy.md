# Agent Note: 多音节候选首词多样性、分段前缀直达与生僻字尾收敛

Status: implemented

## Problem

实机输入「`还真是` + `xianzhezhishi`」时，候选面板出现 `1 / 184` 页，首页前 6 格被单一起点前缀「`闲着`」完全垄断（`1 闲着只是 / 2 闲着之时 / 3 闲着之事 / 4 闲着知识 / 5 闲着知事 / 6 闲着`），「`贤者之石`」在全部候选页中完全消失；即使放弃整句、手动分段打出「`贤者` + `之` + `石`」，前缀词「`贤者`」也被挤出首页，进入 `zhishi` 后「`之石`」落在第 2 页且单字「`之`」被压至第 9 页，单音节 `shi` 中「`石`」也因第 14 位超出 `MAX_RERANK_PATHS = 12` 而未在 Tier 0 获得 `.gram` 语境提权；同时热态 `Qwen3-0.6B` 16 候选前缀树及短句受限束搜索真实端侧耗时（`30–67 ms`）超出原 `28 ms` `Provisional` 防闪窗，导致长句槽 0 在回包抵达前提前锁死。

根因分布在词网格解码、排座配额、Tier 1 神经融合/防闪时序与尾部收集五处契约耦合：

1. **词网格中间列单前缀垄断、末列首词无约束与 `[1, 1] -> [1, 1]` 联合扰动缺失（`crates/engine/src/lattice/decode.rs`）**：上文「`还真是`」下「`闲着`」在第 2 列领先后，展开 `zhi`/`shi` 高熵列（40+ 单字出边）时每个父节点无限扇出，把「`贤者 + 之`」挤出 `eff_beam = 56`；当主路径呈 `[2, 1, 1]`（`闲着+知+事`）时，混淆网络在 `pair_len == 2` 上仅尝试 `[1, 1] -> [2]` 词典合并而缺少 `[1, 1] -> [1, 1]` 双向 `.gram` 联合重解码，也缺少从 `graph[0]` 合成多字首词锚点的通道，使「`贤者之石`」无法在 Tier 0 与 `.gram` 强搭配汇合。
2. **未测频占位长词阻塞纠错防篡夺与分段前缀后置（`crates/engine/src/lattice/merge.rs`、`quanpin/dictionary.rs`、`ordering.rs`）**：`weight == 100` 的 3–4 字占位词（如 `先哲之石`）既会以弱权重抢占 `Block` 前方、在后置时又因 `has_exact_literal_dict_leader` 只检查首项而意外放行误纠行（`想着只是`）篡夺第 0 槽；同时 `FRONT_NON_READING_LIMIT` 曾让 20+ 条合成次选整句插在前缀短词与常用单字之前，双音节 `MAX_LEADING_EXACT_TWO_SYLLABLE_DICT = 6` 也延后了「`之石`」与「`之`」的直达位置。
3. **Tier 1 神经融合缺少 `2+1+1` 首词多样性、隔音误判与 `28 ms` 早锁（`neural_v2.rs`、`session/candidates.rs`、`host/session.rs`、`input-runtime`、`ShurufaNeuralEngine.cpp`）**：隔音膨胀句（`xi'an'zhe'zhi'shi -> 西安这只是`）误享多字首词地位；`take_neural_request` 扫描上限 `64` 与单音节 `cand_cap` 会漏收顺延等跨度句与第 13+ 位同音常字；`NEURAL_PROVISIONAL_WINDOW = 28 ms` 频繁在热态推理完成前锁死 Tier 0 错误首槽（如 `'我们去吃饭把'`、`'充个点'`）。
4. **多音节组字全量倾倒零频古汉字与非主切分单字（`crates/engine/src/session/candidates.rs`）**：将 `tbl_1` 中数百个 `weight <= 0` 扩展区古汉字与隔音派生单字全部送入展示列表，造成 `184` 页尾部噪声。

## Decision

围绕「**多前缀并存搜索 → 首屏 `4+2` 读法与前缀共存（`2+1+1` 成词首词优先） → 第 2–4 页前缀直达与常用字优先 → 有界尾部净化**」，统一收紧五层契约：

1. **词网格中间列防垄断、多锚点混淆网络与 `.gram` 联合重解码（`crates/engine/src/lattice/`）**：
   - `decode.rs` 在产品模式（`options.terminal_max_per_suffix > 0`）下：`terminal_beam = 224`、`terminal_limit` 放大至 `eff_beam * 4` 封顶；中间列向同一目标列偏移 `1..=8` 展开且仅当 `dist == 1 || edge.end == n` 时，每条父假设按栈内定长缓冲限流保留得分最高的 `PER_PARENT_DEST_CAP = 8` 条出边，阻止领先前缀通过单字表扇出把次优前驱挤出 `eff_beam`；`CN_MAX_LOCAL_DROP` 放宽至 `26.0` nats。
   - 末列第一遍（`pass1_limit = take - 3`）将全 `[1, 1, ...]` 起手路径限流在 `MAX_LEADING_SINGLE_PAIR_PASS1 = 4` 条，第二遍（novel skeleton）同受 `pass1_limit` 约束，并在第三遍一般 `deferred` 回填之前专设 `MULTI_CHAR_HEAD_RESCUE_LIMIT = 3` 多音节首词保底通道（`margin.max(26.0)`），确保次优多字首词进入主束。
   - 混淆网络 `augment_with_confusion_network_perturbations` 先从 `paths[1..10]` 吸收不同多字首词锚点，不足时由 `build_multi_head_seed_anchor` 直接从 `graph[0]` 取前 3 条高先验多音节边对齐主路径后段边界合成种子路径（并在 `round = 1` 入选 `ranked`），将全 `[1, 1, ...]` 起手单字扰动轮次后移 1 轮；`collect_two_edge_splits`（算子 B）覆盖 `li < 6` 且配额扩为 `(10, 28, 16, 12)`，同时在相邻双单字 `pair_len == 2` 下新增**算子 C2 `[1, 1] -> [1, 1]` 双向 2-stage 联合重解码**，解锁需两字协同 `.gram` 搭配才能成立的非词典组合（如 `贤者 + 之 + 石`）；入选前 16 的 CN 胜出路径若满足前 6 席的 `terminal_max_per_suffix` 与 `terminal_max_per_prefix + 1` 配额且得分超过当前第 6 席，则轮转上浮至前 6 席按分排序，既保住多样性防后缀塌缩，又消除束剪枝导致的 `.gram` 胜出句倒挂。
   - `merge.rs` 将双音节领跑精确词阈值 `MAX_LEADING_EXACT_TWO_SYLLABLE_DICT` 收敛为 `4`；新增 `whole_sentence_insert_position_with_options`，在 3–4 字输入且存在汉字上文与 `.gram` 打分器时，将 `Database` 中 `weight == 100` 的未测频占位长词（如「`先哲之石`」）后置于真实搭配整句之后；`quanpin/dictionary.rs` 修复 `has_exact_literal_dict_leader`，检查首条非生成句词条，防止长词后置后触发误纠路径（如 `'想着只是'`）反抢第 0 槽。
2. **多音节单字上下文精选与长尾噪声边界（`crates/engine/src/lattice/octagram.rs`、`crates/engine/src/session/candidates.rs`）**：
   - 将 `MAX_RERANK_PATHS` 扩至 `24`、`HOMOPHONE_SCAN_WINDOW` 扩至 `48`，并在多音节带汉字上文输入时对首音节前 `24` 个常用单字运行 `.gram` 上下文精选；非 `Tab` 笔画筛选且非单字模式的常规多音节拼音组字下，剔除非主切分隔音派生单字、当首音节单字超过 `24` 条时过滤 `weight <= 0` 零频古汉字并设常用单字上限 `64`，非词典生成整句收紧至 `MAX_GENERATED_SENTENCES = 16`。
3. **Tier 0 与 Tier 1 对齐的 `4+2` 读法配额与 `2+1+1` 成词首词多样性（`pipeline.rs`、`host/session.rs`、`ordering.rs`、`neural_v2.rs`、`input-runtime`）**：
   - 在 `CandidateEntry` / `OrderRow` / `EngineNeuralCandidateMeta` 引入 `lexical_head` / `has_lexical_head`（仅当 `extra_hiatus == 0` 即非隔音膨胀切分，且为词典整词或首段切分词 `>= 2` 字时为真），排除 `xian'zhe'zhi'shi -> xi'an'zhe'zhi'shi`（「`西安这只是`」）冒充多字首词读法。
   - Tier 0 (`ordering::visit_runner_up_order`) 与 Tier 1 (`neural_v2::fuse_neural_response`) 在 3+ 字输入下统一执行 **首屏 `4+2`（4 条多样化整句 + 2 席前缀直达）+ `2+1+1` 3 遍选拔**：第一遍（`Pass 0`）主首词最多取 `2` 席（纠错领跑时取 `1` 席），其余读法仅接纳满足 `has_lexical_head`（`lexical_head`）且进入门槛分数线的异首词候选（每种首词 `1` 席），优先确保前 4 句至少包含 2 种以上不同多字词首词；不足时第二遍（`Pass 1`）接纳非成词首词异首读法；第三遍（`Pass 2`）按分补足余下整句席位；第 2–4 页紧接 `FRONT_NON_READING_LIMIT = 24` 席前缀短词与常用单字，双音节前 8 席后穿插 `6` 席常用单字前缀。
   - `crates/input-runtime/src/runtime.rs` 将 `take_neural_request` 的非游离基础候选扫描上限从 `64` 扩至 `128`，单音节输入放宽至 `24` 条（`platforms/macos/src/neural/ShurufaNeuralEngine.cpp` 同步对齐 `primary_n == 1` 下 `cand_cap = 24`），并跳过 `stat_log_prob.is_none()` 的未测频占位词对 `exact_dict_len_mask` 的误触发；将 `NEURAL_PROVISIONAL_WINDOW` 从 `28 ms` 调整至 `70 ms`、`MAX_FAST_SPACE_WAIT` 从 `35 ms` 调整至 `55 ms`，以覆盖 16 候选前缀树 Stage A + 短句受限束 Stage B 的实机 Metal 热推理周期（`30–67 ms`），避免过早锁死槽 0。

## Historical audit

检索 `.agents/notes/` 后，本决策与以下既有笔记存在直接演进或局部取代关系：

- [词网格指物代词对等、末列前缀多样性、隔音切分降级与次选排座精排](../../implemented/bug-fix/2026-11-14-lattice-pronoun-diversity-and-seating.md) 与 [词网格动宾离合词拆插补边与三字短词悬殊次选首屏让位](../../implemented/bug-fix/2026-11-18-separable-vo-compounds-and-runner-up-gating.md)：延续其首屏整句与前缀共存框架和 `Terminal` 多样性思路，将次选合成整句进一步后置到 `24` 条前缀区之后、双音节精确词阈值收敛至 `4` 并增加 `2+1+1` 首词多样性配额。
- [Shurufa v2 统一概率词格与端侧 Metal 两阶段神经解码架构](./2026-11-24-shurufa-v2-probabilistic-neural-ime.md)：延续统一自然对数词格、`Qwen3-0.6B` 双边界概率与 `4+2` 读法布局；补齐中间列按父假设 `PER_PARENT_DEST_CAP = 8`、混淆网络多锚点种子与算子 C2 `[1, 1] -> [1, 1]`、`fuse_neural_response` 的 `2+1+1` 首词多样性及未测频占位词护栏，并将 `MAX_RERANK_PATHS` 扩至 `24`、`NEURAL_PROVISIONAL_WINDOW` 调至 `70 ms`。
- [Shurufa 纯 macOS 单产品减法与三段单向候选管线收敛](./2026-11-25-shurufa-pure-macos-pipeline-convergence.md)：严格保持 `CursorSnapshot -> RecallPool -> UnifiedScorer -> PurePresenter` 单向数据流与 `SlotAllocator` / L4 固定排位不变量，`has_lexical_head` / `lexical_head` 仅作为前段召回元数据传递，不破坏无状态 `PurePresenter` 契约。

## Alternatives considered

### 1. 仅放宽词网格全局 `beam` 到 `128+` 而不做中间列按父假设单字扇出限流

- **最强理由**：实现仅需修改一个常量，理论上所有低权前缀都有更多机会存活到末列。
- **放弃原因**：`zhi`/`shi` 等单字表前 40 项高频字均落在远高于中频双字词的局部权重区间，单靠扩大全局束宽只是成倍增加计算量，单个领先前缀仍会在高熵列按组合数吞掉绝大部分束位；在中间列仅对单字转移边按父假设限流为 Top-8（`PER_PARENT_DEST_CAP = 8`）并辅以 `build_multi_head_seed_anchor` + 算子 C2 `[1, 1] -> [1, 1]`，可在低开销下从结构上保证多首词路径与中频 `.gram` 搭配并存。

### 2. 在 `PurePresenter` 末级截断或重新洗牌，而不改动 `ordering::runner_up_order` 与 `fuse_neural_response`

- **最强理由**：仅改展示投影层一处文件，不影响 Tier 0 无神经路径或 `convert_eval`。
- **放弃原因**：只在末尾展示层重写次序会撕裂 Tier 0 首帧与 Tier 1 融合回包之间的排座权威，导致在 `Provisional -> Presented` 期间或关闭神经服务时候选布局退化；必须在 `decode.rs`（源头多样化）、`ordering.rs`（Tier 0 统一排座）与 `neural_v2.rs`（Tier 1 融合排座）共享同构契约。

### 3. 将首屏读法配额缩减至 `3+3` 并严格按 `[1, 2, MAX]` 禁止同首词第二候选

- **最强理由**：首页固定让出第 4–6 格给 3 条前缀词，静态场景下前缀词曝光更早。
- **放弃原因**：实机在 `convert_eval` 评测上，严格禁止同首词第二候选导致 `sentences-v2.tsv` 的 `immediate_top6` 从 `0.801` 跌至 `0.788`、`shurufa-spoken-v1.tsv` 从 `0.900` 跌至 `0.892`，因为「`充个点/充个电`」「`在一起/再一起`」「`大约/大悦`」等共享前 2 字、仅靠尾字 `.gram`/语境分胜出的关键同音竞争对被强行挤出首页；改为 `4+2` 读法配额配合 `2+1+1` 3 遍选拔（允许主首词占 `2` 席但必须先给成词异首词预留多样席位）后，`shurufa-spoken-v1.tsv` 升至 `0.917`、`sentences-v2.tsv` 升至 `0.837` 且「`闲着`/`贤者`」仍稳居首页第 5–6 格。

## Consequences

- `convert_eval --page-size 6 --autocorrect --attribution`（配 `wanxiang-lts-zh-hans.gram`）实测：`resources/eval/shurufa-spoken-v1.tsv`（120 例）`immediate_top1 = 0.750`、`immediate_top6 = 0.917`、`mrr = 0.818`、`search_pruned = 7`、`dict_oov = 0`；`resources/eval/sentences-v2.tsv`（312 例）`immediate_top1 = 0.599`、`immediate_top6 = 0.837`、`mrr = 0.708`、`search_pruned = 31`、`engine_rank_gt6 = 12`、`presentation_demoted_off_page1 = 8`、`dict_oov = 0`。
- 端侧 `ShurufaNeuralService`（`Qwen3-0.6B-Base-Q4_K_M.gguf`）实机 C ABI 9 组回归验证（人手输入节拍）：
  - 「`还真是` + `xianzhezhishi`」总页数由 `184` 页收敛至 `15` 页（`89` 条），Tier 0 首页 `['想着只是', '贤者之石', '闲者知事', '闲着知识', '闲着', '贤者']`（`贤者之石` `#2`），Tier 1（`45.7 ms`, `slot0_locked = false`）首页 `['想着只是', '闲着之时', '贤者之石', '闲者知事', '闲着', '贤者']`（`贤者之石` `#3`，`闲着`/`贤者` `#5–#6`，`先哲` `#7`）。
  - 分步路径「`还真是贤者` + `zhishi`」中「`之石`」居 Tier 0 `#6` / Tier 1 `#2`；「`还真是贤者之` + `shi`」中「`石`」在 Tier 0 与 Tier 1 均居 `#1`。
  - 控制用例全部守住或提升：`womenquchifanba` Tier 1 `#1 我们去吃饭吧`；`alibabajituan` Tier 0/1 `#1 阿里巴巴集团`；`gedeerde` Tier 0/1 `#1 哥德尔的`；`tamen` 首页含 `它们`（Tier 1 `#2`）；`bata` 首页含 `把它`（Tier 1 `#2`）；`chonggedian` Tier 1 `#1 充个电`。
- **代价与后续跟进**：
  - 将 `NEURAL_PROVISIONAL_WINDOW` 从 `28 ms` 放宽至 `70 ms` 后，复杂 12+ 音节长句在不触发快敲空格的前提下最多延后 `~70 ms` 首绘面板，以换取热态 `40–65 ms` 神经推理不锁槽 0；快敲空格仍通过 `MAX_FAST_SPACE_WAIT = 55 ms` 阻塞合并，未暖机直接 0 ms 跳过。
  - 多音节常规中文组字在首音节单字超 `24` 条时跳过 `weight <= 0` 扩展区生僻单字，用户若需在长拼音串中显式输入极生僻古汉字，可切回单字输入或使用 `Tab` 笔画/部件筛选（两条通路仍保留全量单字表）。
  - Tier 0「`还真是` + `xianzhezhishi`」中同音误纠路径「`想着只是`」因 `xian'zhe'zhi'shi` 先验略低于 `'想着'` 首字链仍居首页第 1 位，后续可通过优化误纠信道惩罚与 `.gram` 句首上下文校准进一步压降。
