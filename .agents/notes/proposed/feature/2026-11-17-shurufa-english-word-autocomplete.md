# Agent Note: Shurufa 中文输入下的英文单词自动补全

Status: proposed

## Problem

Shurufa 在 [对齐系统拼音的输入边界](../../implemented/simplification/2026-10-29-shurufa-system-pinyin.md) 时移除了内部自管英文模式，同时把运行时副本中的混合候选总开关（`mixed_candidates`）与临时英文模式一并关闭，要求英文一律通过系统 Caps Lock 切到 macOS ABC 输入。

日常中文写作与沟通里经常夹带单个英文单词（如 `config`、`Claude`、`interface`、`hello`）。每打一个词都来回按两次 Caps Lock 会打断句子节奏，而 macOS ABC 输入源本身也没有离线英文词典前缀补全。即便直接把桌面既有的 `mixed_input.english` 打开，桌面宿主仍固定 `minimum_prefix = 5` 且默认 `first_page_count = 1`（见 [混输总开关与首页英文候选数](../../implemented/feature/2026-10-22-mixed-input-master-switch-and-first-page-english.md)）：

1. 2～4 个字母的前缀只做完整词精确查找（见 [短英文完整匹配](../../implemented/bug-fix/2026-10-20-short-exact-mixed-english.md)），输入 `imp`、`conf`、`func`、`eng` 时不会给出任何前缀补全候选；
2. 达到 5 个字母后，只要当前串本身恰好是一个完整英文词（如 `inter`、`config`、`model`），首页只留这 1 个英文座位，更长的补全词（`interface`、`configuration`、`models`）会被排到全部中文单字末尾；
3. 首字母大写或驼峰输入（如 `Cla`、`Git`、`Mac`）在关闭临时英文模式后只能原样上屏，没有补全候选可选。

## Proposal

在保持 Shurufa「无内部英文模式、Caps Lock 仍归系统切换 ABC」边界的前提下，利用随包已有的 `msime-english.db` 为中文组字补齐英文单词自动补全：

1. **Shurufa 运行时单独放开英文混输补全**：在 Host API 的 Shurufa 运行时副本中启用英文混输（保留 emoji、颜文字、快捷短语、日文混输与行内日期关闭），并将首页英文候选数默认值调为 `2`～`3`，使精确匹配之外还能同屏展示高频前缀补全词。
2. **区分大写起手与全小写混输前缀门槛**：
   - 首字母大写或含大写字母的纯 ASCII 字母串（且非激活辅助码）：从 2～3 个字母起直接查询 `msime-english.db` 前缀补全并按用户输入的大小写形态投影候选（如 `Cla` → `Claude`、`Class`），原文与最佳补全排在首页前排；
   - 全小写输入：完整拼音可拆通时继续保持中文候选居首（遵守 [混输英文占首选](../../implemented/feature/2026-10-12-mixed-english-leads-unsegmentable-input.md) 的不抢位规则），对无法拆成合法完整拼音音节的 3～4 字母前缀允许在非首选优先座位展示 1～2 条高频英文前缀补全，5 字母及以上拆不开且完全匹配时维持英文占首选并在其后紧跟补全词。
3. **上屏手势保持一致**：回车始终原样上屏已输入字母，空格提交当前高亮候选，数字键与方向键可直接点选补全词，不因展示补全词而在中文简拼上误扩写英文。

## Historical audit

- [Shurufa 对齐系统拼音的输入边界](../../implemented/simplification/2026-10-29-shurufa-system-pinyin.md)：部分重叠。保留其「不设内部英文模式、不接管语言切换快捷键、关闭表情/快捷短语等杂项混输」的核心边界；落地本提案时仅松开其中「关闭英文混合候选」这一条，并原地互链。
- [混输总开关与首页英文候选数](../../implemented/feature/2026-10-22-mixed-input-master-switch-and-first-page-english.md)、[拼音拆不开且英文完全匹配时，混输英文占首选](../../implemented/feature/2026-10-12-mixed-english-leads-unsegmentable-input.md) 与 [短英文完整匹配](../../implemented/bug-fix/2026-10-20-short-exact-mixed-english.md)：部分重叠。复用其词典连接、音节可拆判定、大小写投影与首页英文座位机制，针对 Shurufa 补齐短不可拆前缀与大写起手的补全召回。
- [Android 英文候选恢复前缀补全](../../implemented/bug-fix/2026-10-23-android-english-prefix-completions.md)：面向 Android 独立英文键盘，路径独立，但其「展示补全而不让空格把未打完前缀强行扩写」的交互约束一致。
- [统一多语言组字提案](2026-10-24-unified-language-composition.md)：讨论跨语言整句网格切分，范围远大于单段英文词前缀补全，互不阻塞。

## Alternatives considered

- **恢复内部英文候选模式（`dedicated_english`）及切换快捷键**：能直接复用引擎现成的两字母英文补全路径，但违背 Shurufa 只用系统输入源切换中英文的产品定位，用户仍需多记一套内部状态。
- **直接把全平台桌面 `minimum_prefix` 全局降到 2 或 3 且不做音节拆分过滤**：实现最简单，但会让 `dj`（大家）、`cd`（成都）、`cs`（测试）、`bus`（不是）等大量中文简拼在首页混入乃至被英文缩写干扰；必须结合完整音节/简拼冲突过滤控制展示时机与座位。

## Acceptance criteria

- 在 Shurufa 中文输入状态下敲 `conf`、`inter`、`Cla` 等英文前缀时，第一页候选窗内出现对应的完整英文单词补全（如 `config` / `configuration`、`interface`、`Claude`），点选数字键或方向键可整词上屏。
- 敲 `dajia`、`nihao`、`cd`、`dj` 等正常中文全拼与高频简拼时，中文首选不被英文词抢占；敲未打完的英文前缀按回车仍原样上屏已敲字母。
- 空闲未敲键时不弹出任何候选窗；Caps Lock 切到系统 ABC 的行为完全不变。

## Risks

- 3～4 个字母的小写串与部分不常见中文简拼（如生僻词首字母缩写）存在交集：若把补全放在第 2 位，原本排第 2 位的中文简拼候选会顺延一格，落地时需用真实拼音简拼样本核对首页座位影响。
- 全拼辅助码关闭（Shurufa 默认关闭）时大写字母一律可视作英文起手，但若用户手动开启了大写辅助码（如 `niC`），首字母小写而末尾大写必须继续归辅助码，只在首字母大写或辅助码关闭时走大写英文补全。
