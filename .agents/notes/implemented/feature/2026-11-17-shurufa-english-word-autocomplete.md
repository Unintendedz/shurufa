# Agent Note: Shurufa 中文输入下的英文单词自动补全

Status: implemented

## Problem

Shurufa 在 [对齐系统拼音的输入边界](../simplification/2026-10-29-shurufa-system-pinyin.md) 时移除了内部自管英文模式，同时把运行时副本中的混合候选总开关（`mixed_candidates`）与临时英文模式一并关闭，英文一律通过系统 Caps Lock 切到 macOS `ABC` 输入。

日常中文写作与沟通里经常夹带单个英文单词（如 `config`、`Claude`、`interface`、`hello`）。每打一个词都来回按两次 Caps Lock 会打断句子节奏，而 macOS `ABC` 输入源本身也没有离线英文词典前缀补全。若只把桌面既有的 `mixed_input.english` 直接打开，桌面宿主仍固定 `minimum_prefix = 5` 且默认 `first_page_count = 1`（见 [混输总开关与首页英文候选数](2026-10-22-mixed-input-master-switch-and-first-page-english.md)）：

1. 2～4 个字母的前缀只做完整词精确查找（见 [短英文完整匹配](../bug-fix/2026-10-20-short-exact-mixed-english.md)），输入 `imp`、`conf`、`func`、`eng` 时给不出任何前缀补全候选；
2. 达到 5 个字母后，只要当前串本身恰好是一个完整英文词（如 `inter`、`config`、`model`），首页只留这 1 个英文座位，更长的补全词（`international`、`configuration`、`models`）会被排到全部中文单字末尾；
3. Shurufa 为阻止 `Shift+V` 等进入本地快捷模式而关闭了 `shift_letter_entry`，导致未组字时按 `Shift+字母` 直接透传宿主，首字母大写或驼峰输入（如 `Cla`、`Git`、`Mac`）无法在同一组字内触发英文词典前缀补全。

## Decision

在保持 Shurufa「无内部英文模式、Caps Lock 仍归系统切换 `ABC`」边界的前提下，利用随包已有的 `msime-english.db` 为中文组字提供英文单词自动补全：

1. **Shurufa 运行时单独放开英文混输与短前缀补全**：`crates/host-api` 的 `apply_shurufa_input_policy` 在运行时副本中将 `mixed_input.enabled` 与 `mixed_input.english` 固定为 `true`、`mixed_input.first_page_count` 固定为 `3`，保留 `emoji`、`kaomoji`、`symbols`、`japanese`、`quick_phrase` 与 `local_modes.date_time_inline` 全部为 `false`；同时在 `EngineOptions` / `EnglishInputOptions` 新增默认关闭的能力位 `short_prefix_completion`（Host API 仅对 `shurufa` 设为 `true`），设置页与磁盘偏好文档格式不变。
2. **区分大写字面组字与全小写混输前缀门槛**：
   - **含大写字母的纯 ASCII 字母串（且非激活辅助码）**：开启 `short_prefix_completion` 时，即使 `shift_letter_entry` 关闭，全拼与双拼未组字下的 `Shift+字母` 仍起手进入普通字面组字（不触发任何 `Shift+字母` 本地快捷模式）。光标在末尾且长度不少于 2 时直接调用 `EnglishDictionary::query_prefix` 查询前缀补全，经 `InputSession::apply_english_word_case` 按键入大小写形态投影并按投影后的显示文本去重（消除词库中 `python` / `Python` 等同词异形重复）：若命中精确单词则置于第 0 位并在其后紧跟最多 `first_page_count - 1` 条补全；若尚为未完成前缀则将原文字面行（`Fallback`）置于第 0 位，其后紧跟最多 `first_page_count` 条高频补全，避免 `C啦`、`M阿c` 等拼凑单字挤占首页；若词库无任何英文前缀命中（如 `Bchao`、`Txu`、`Fnihao`），保持原有的中文拼接候选居首。
   - **全小写输入**：2 字母输入继续只查 `query_exact` 完整词（保护 `dj`、`cd`、`cs`、`bj`、`sh` 等两字母高频简拼不展开英文补全）；完整拼音音节可拆通的输入（如 `nihao`、`dajia`、`can`、`women`）继续只按原门槛规则处理并保持中文候选居首（遵守 [混输英文占首选](2026-10-12-mixed-english-leads-unsegmentable-input.md)）；无法拆成完整音节且无激活辅助码、无光标前缀解码的 3～4 字母前缀查询 `query_prefix`，不抢占中文第 0 位，首页在可选精确匹配之外最多跟进 2 条补全词（上限不超过 `first_page_count`）；5 字母及以上拆不开且完全匹配时维持英文占第 0 位并在其后紧跟最多 2 条补全词。
3. **句子重排与上屏手势保持一致**：`crates/engine/src/ordering.rs` 的 `row_answers_key` 排除 `EnglishDictionary` 与 `Fallback` 行，使逐键神经模型与停顿精排既不会将中文拼凑候选旋至大写字面原文（如 `Cla`）之前，也不会因混输英文缺少词格静态证据而中断中文整句的静态+神经混合重排。回车（`MSIME_COMMIT_RAW`）始终原样上屏已敲字母，空格提交当前高亮候选，数字键与方向键可直接点选补全词，空闲未敲键时不产生任何候选。

## Historical audit

- [Shurufa 对齐系统拼音的输入边界](../simplification/2026-10-29-shurufa-system-pinyin.md)：部分重叠。保留其「不设内部英文模式、不接管语言切换快捷键、关闭表情/快捷短语等杂项混输」的核心边界；本篇仅松开其中「关闭英文混合候选」这一条，已在原笔记就地互链。
- [混输总开关与首页英文候选数](2026-10-22-mixed-input-master-switch-and-first-page-english.md)、[拼音拆不开且英文完全匹配时，混输英文占首选](2026-10-12-mixed-english-leads-unsegmentable-input.md) 与 [短英文完整匹配](../bug-fix/2026-10-20-short-exact-mixed-english.md)：部分重叠。复用其词典连接、音节可拆判定、大小写投影与首页英文座位机制，通过 `short_prefix_completion` 为 Shurufa 补齐短不可拆前缀与大写起手的补全召回，其他版本行为保持不变，三篇均已补齐互链。
- [Android 英文候选恢复前缀补全](../bug-fix/2026-10-23-android-english-prefix-completions.md)：面向 Android 独立英文键盘，路径独立，但其「展示补全而不让空格把未打完前缀强行扩写」的交互取舍一致。
- [在同一组字中按用户选择的语言连续转换](../../proposed/feature/2026-10-24-unified-language-composition.md)：讨论跨语言整句网格切分，范围远大于单段英文词前缀补全，互不阻塞。

## Alternatives considered

- **恢复内部英文候选模式（`dedicated_english`）及切换快捷键**：最强理由是能直接复用引擎现成的两字母英文补全路径，无需区分简拼与不可拆前缀。未采用，因为这违背 Shurufa 只以系统输入源切换中英文的产品定位，用户仍需额外记忆输入法内部状态。
- **直接把桌面 `minimum_prefix` 全局降到 2 或 3 且不做音节拆分过滤**：最强理由是无需新增引擎能力位，改动最少。未采用，因为会让 `dj`（大家）、`cd`（成都）、`cs`（测试）等两字母简拼展开英文前缀列表，也会让 `can`、`man`、`xian` 等完整拼音音节在首页混入大量英文补全，并且同时改变 `full` 版既有的桌面出厂契约。
- **直接打开 Shurufa 的 `shift_letter_entry` 以恢复大写起手组字**：最强理由是不必修改 `InputSession::dispatch_character` 的 `starts_pinyin_with_shift` 条件。未采用，因为 Shurufa 为支持全拼小写 `v` 计算模式将 `local_modes.expression` 固定为 `true`（见 [Shurufa 恢复计算与数字](2026-11-04-shurufa-lowercase-v-expression.md)），打开 `shift_letter_entry` 会让 `Shift+V` 误入计算模式而无法输入 `Vue`、`Vite` 等 `V` 开头英文词。

## Testing

- 引擎单测（`cargo test -p msime-engine --lib`）：
  - `ime::queries::tests::short_prefix_completion_expands_unsegmentable_three_and_four_letter_lowercase_without_leading` 覆盖 `dj`（仅完整词）、`imp`（2 条补全居次席）、`conf`（精确词加 2 条补全）、`can`（完整音节不展开补全）、`english_may_lead = false`（辅助码/光标前缀不展开）与 `inter`（5 字母精确匹配占首选并带补全）。
  - `session::tests::shurufa_short_prefix_completion_handles_uppercase_lowercase_and_helpcode_boundaries` 覆盖 `shift_letter_entry = false` 下的大写起手组字（`Cla` 原文居首、点选补全、回车原样上屏）、同词大小写投影去重（`Pyth`）、精确大写词（`Mac`）、无英文命中的大写接中文（`Fnihao` → `F你好`）、全拼辅助码关闭与开启时 `niC` 与 `NiC` 的分流，以及 `nihao`、`dajia`、`dj`、`cd`、`conf`。
  - `ordering::tests::reranker_preserves_literal_fallback_leaders_and_ignores_mixed_english_rows` 验证 `Fallback` 字面首选不被逐键/停顿模型推翻，且混输英文行不打断中文词格静态证据评分。
- 宿主接口与真实随包词库回归（`SHURUFA_TEST_RESOURCES=target/macos/EngineResources cargo test -p msime-host-api --lib -- shurufa_tests`）：
  - `removed_input_modes_stay_unreachable_after_legacy_preferences_and_updates` 验证旧偏好更新后仍只开启英文混输与 `first_page_count = 3`，其余混输分项与 `Shift+字母` 模式全部保持不可达。
  - `shurufa_english_autocomplete_gestures_and_helpcode_gate` 与 `bundled_input_learning_and_neural_model` 覆盖空闲无候选、方向键移动加空格提交、数字/索引点选、回车原样上屏、开启大写辅助码时的 `niC` 门禁，以及随包真实 `msime-english.db` / `msime-pinyin.db` 上的候选表现。
- 真实词库简拼样本核对：在随包 `msime-pinyin.db` 与 `msime-english.db` 上对照常用 3～4 字母简拼，`wsm`（为什么）、`zfb`（支付宝）、`sfz`（身份证）、`gjj`、`xyk`、`pyq`、`gzh`、`xcx`、`bzd`、`zmy`、`rgzn`（人工智能）、`mywt`（没有问题）在英文库中均为 0 命中；存在交集的 `sms`（什么是，1 条 `SMS`）、`pdf`（配电房，2 条 `PDF`/`PDFs`）、`ssh`（说实话，2 条 `ssh`/`SSH`）与 `bus`（不是）均保持中文第 1 位不变，6 格首页仍保留 3～5 个中文席位。

## Consequences

- **收益**：Shurufa 中文输入下无需来回按 Caps Lock 即可获得 3～4 字母不可拆前缀（如 `conf` → `conf` / `conference` / `configuration`，`imp` → `important` / `improve`）、5 字母以上前缀（如 `inter` → `inter` / `international` / `internet`，`interf` → `interface`）及大写起手（如 `Cla` → `Class` / `Classic`，`Clau` → `Clause` / `Claude`）的离线英文补全；中文全拼、两字母简拼与回车原样上屏手势保持不变。
- **代价与已知边界**：
  - 未组字时按 `Shift+字母` 在 Shurufa 中进入字面组字（预编辑显示该大写字母，空格或回车原样上屏），若紧接数字键 `1`～`6` 会走候选窗选词而非直接插入数字；输连续编号或纯英文长句仍应通过 Caps Lock 切至系统 `ABC`。
  - 英文补全按 `msime-english.db` 既有的词频权重排序（如 `Cla` 前三项为词频更高的 `Class`、`Classic`、`Classes`，多敲一键 `Clau` 即在首页前排给出 `Clause`、`Claude`；`inter` 前三项为 `inter`、`international`、`internet`，`interf` 即给出 `interface`），本次不改动随包词库权重，且混输下点选英文不改写词频。
  - 3～4 字母小写不可拆输入若恰好与生僻中文简拼同形且命中英文前缀（如 `sms`、`pdf`、`ssh`），原本排第 2～3 位的次选中文简拼会后移 1～2 格；若后续实际使用中发现特定简拼受扰，可考虑引入元音或辅音组合过滤。
  - 全拼辅助码在 Shurufa 默认关闭时 `niC` 等含大写串一律走英文补全；用户手动开启全拼辅助码后，首字母小写且末尾 1～2 码大写的输入继续归辅助码过滤，只有首字母大写时才走英文补全。
