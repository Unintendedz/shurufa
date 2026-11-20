# Agent Note: Stage 1 纯函数 SlotAllocator 与重排后精选内联 Emoji 投影

Status: implemented

## Problem

Shurufa 需要支持类似搜狗、微信输入法的「打字出 Emoji」（如打 `sikao` 在首屏给出「思考」和「🤔」），但直接打开既有 `mixed_input.emoji`（位于 `crates/engine/src/ime/queries.rs`）会同时触发多项结构性冲突：

1. **重排撕裂锚定**：逐键神经重排（`rerank_pick`）、停顿精排（`settled_rerank_order`）与次选整句降位（`runner_up_order`）都运行在 `anchor_expressive_rows` 之后，停顿精排原地交换中文下标会把 Emoji 贴到错误中文词后面，`runner_up_order` 还会把 Emoji 算进前排非读法配额挤飞正常中文词与英文补全。
2. **与英文自动补全撞车**：`anchor_expressive_rows` 匹配英文关键词，会在 `cat`、`ok`、`fire` 等英文词与其 `short_prefix_completion` 补全之间楔入 Emoji。
3. **热路径前缀扫描与裸 CLDR 噪声**：`local/emoji.rs` 使用无 `LIMIT` 的 SQLite 前缀范围扫描（`p.key >= ?1 AND p.key < ?2`），单锚点无上限，且裸 CLDR 词表包含 `一→1️⃣`、`不→🙅‍♂️🙅‍♀️`、`好→👌🙆‍♂️🙆‍♀️`、`得→🉐`、`有→🈶` 等高频虚字、日文方块字与冗余性别双胞胎，同时缺失现代中文聊天别名（如「摸鱼」「破防」「收到」「晚安」「打卡」）。
4. **光标前缀吞字与零本地记忆**：旧路径漏检 `prefix_active` 与 `has_active_helpcode()`，且永远按静态 `sort_order` 死排、不记住用户对同一词偏爱的 Emoji。

## Decision

按照 [Shurufa 候选词决策链五层重构与分阶段演进](../../../proposed/architecture/2026-11-20-shurufa-candidate-pipeline-redesign.md) 的 Stage 1 契约，将 Shurufa 内联 Emoji 从旧 `queries.rs` 拼音前缀混排彻底解耦，落地为「重排后纯函数槽位投影」：

1. **编译期精选清洗词表（`crates/engine/src/local/curated_emoji.tsv` + `inline_emoji.rs`）**：
   - 不改上游锁定 `msime-others.db` 哈希；仓库内置 4,415 条经过卫生清洗的 `中文词 → [emoji...]` 映射表（`include_str!` + `OnceLock<HashMap>`，O(1) 内存反查，零每键 SQLite 锁与文件排序，全拼/双拼/模糊音/纠错天然一致）。
   - 单测强制守卫五条卫生契约：零肤色修饰符（`U+1F3FB..=U+1F3FF`）、零日文方块汉字（`🉐🈶🈚…`）与带框数字（`1️⃣…🔟`）、零无性别语义的 ZWJ 性别冗余（统一归并到中性基表情如 `🤦`/`🙆`/`🙅`/`🤷`）、零单字国旗、单字仅限 60 个具象/情绪白名单（彻底封禁 `一/不/好/得/有/无/上/下/中` 等虚字与功能字），并按中文社交常用序补齐现代口语别名（`思考→🤔`、`大笑/笑哭→😂 🤣`、`开心→😄 😊 🥳`、`捂脸→🤦`、`点赞→👍`、`收到→👌 ✅`、`晚安→🌙 😴 💤`、`摸鱼→🐟 🐠`、`破防→😭 💔` 等）。
2. **无状态纯函数 `SlotAllocator`（`crates/engine/src/slot_allocator.rs`）**：
   - 位于 `crates/engine`，输入仅接受稳定 `id`、文本、大类 `SlotCandidateKind` 与 `answers_key`，不依赖行号或零散布尔位。
   - 硬性不变量：绝不占下标 0（空格永远提交首位）；英文或字面原文居首时静默；仅检查 Top-2 覆盖完整按键的中文候选词（`answers_key == true`）；主推 Emoji 默认落在下标 2（第 3 位，不足 2 项时紧跟首位后；若下标 1–2 为连续英文补全块则顺移到英文块之后，绝不楔入）；首页至多 1 个主推 Emoji，同词其余备选 Emoji 放第 2 页开头（下标 `page_size`）；剔除投影行后主候选顺序与输入逐条一致。
3. **`input-runtime` 统一后置投影与独立本地 MRU（`crates/input-runtime/src/runtime.rs` + `emoji_mru.rs`）**：
   - 逐键呈现（`order_presentation`）与异步停顿精排（`finish_settled_presentation`）在 `rerank`、`demote_runner_up_readings` 与 `normalize_online_slots` 全部完成后**调用同一纯函数 `SlotAllocator`**；异步/同步停顿精排前自动剥离旧投影行，评分只看干净主候选，精排返回后立即按新 Top-2 重新投影，彻底消除锚定撕裂与对停顿精排的阻塞。
   - 严格门控：仅在 `local_mode == "none"`、`!dedicated_english`、`!prediction`、非整句改字、非光标后编辑、`caret_position == editing_text.len()`（杜绝 `prefix_active` 吞字）、非大写输入（保护大写英文与辅助码）与非 Tab 笔画筛选（`!preedit.contains('｜')`）下生效。
   - 选中投影 Emoji 时直接丢弃引擎组字并原样上屏（跳过 OpenCC 简繁转换），**绝不写主拼音词频或个人转移链**，只记录到独立的 `<user_data>/emoji_mru.json`（上限 512 条，原子写入）；同一词对应多 Emoji 时自动将用户偏爱项提至第 3 位主推席。
4. **Shurufa 策略、原生开关与一次性迁移**：
   - `crates/client-core` 将 `shurufa` 的 `mixed_input.emoji` 默认值设为 `true`；`crates/host-api` 在 `edition == "shurufa"` 下固定保持旧引擎 `options.mixed_emoji = false`，将 `preferences.mixed_input.emoji` 路由至 `Runtime::set_inline_emoji_projection`。
   - `platforms/macos/src/settings/AppearancePreferences.mm` 解禁 `mixedEmojiInput` 读写（修复此前修改其他设置时会悄悄把 `emoji` 覆写回 `false` 的缺陷），并在 Shurufa「输入习惯 → 标点与字符」展示「候选中显示 Emoji」开关；`InputController.mm` 对来源 `6`（Emoji）跳过 OpenCC 简繁转换。
   - `ShurufaPreferenceMigration.h`、`input_method_main.mm` 与 `install-shurufa.sh` 新增一次性迁移 `MSIMEMigrateShurufaInlineEmoji`（`ShurufaInlineEmojiMigrationV1` / `--migrate-inline-emoji`），仅把历史旧安装里被旧策略写死的 `false` 升级为 `true` 一次，之后完全尊重用户手动关闭。

## Historical audit

- [Shurufa 候选词决策链五层重构与分阶段演进](../../../proposed/architecture/2026-11-20-shurufa-candidate-pipeline-redesign.md)：本篇落地其中的 Stage 1（纯函数 `SlotAllocator` 边界与干净内联 Emoji 投影）。
- [Shurufa 对齐系统拼音的输入边界](../simplification/2026-10-29-shurufa-system-pinyin.md)：部分重叠并延续其主边界；旧 `queries.rs` 拼音前缀混排和 `Shift+E` 模式仍保持关闭，仅放开重排后受控槽位内的精选内联 Emoji 投影。
- [Shurufa 中文输入下的英文单词自动补全](2026-11-17-shurufa-english-word-autocomplete.md)：互补；`SlotAllocator` 显式保护英文领衔与连续英文补全区块不被 Emoji 楔入。

## Alternatives considered

### 直接打开旧 `EngineOptions.mixed_emoji` 并在 `queries.rs` 打补丁
最强理由是改动极小。未采用，因为旧路径跑在固定排位、逐键神经重排、停顿精排和 `runner_up_order` 之前，且走热路径 SQLite 前缀扫描与裸 CLDR 表，会直接引发停顿精排锚定错位、英文补全被挤出首页、光标前缀选 Emoji 吞字与高频虚字刷屏。

### 把主推 Emoji 放在第 2 位（下标 1）或允许首页放 2 个
最强理由是少按一格数字键。未采用，因为桌面横排候选栏第 1 位（空格）和第 2 位（高频同音二选）是中文盲打黄金区；固定在第 3 位（下标 2）且首页限 1 个，既一眼可见、按 `3` 即可上屏，又保护前两位盲打肌肉记忆，同时把备选 Emoji 放在第 2 页开头配合本地 MRU 解决多表情切换。

## Consequences

- **收益**：Shurufa 在全拼与双拼下打中文词即可在第 3 位稳定获得贴合中文社交习惯的 Emoji（如 `sikao → 思考 / 🤔`），永不抢占空格首选、不被停顿精排撕裂、不挤坏英文补全、不污染拼音词频，且可通过本地 MRU 自动记住个人偏爱表情、通过原生设置页随时启停。
- **代价与边界**：
  - 本阶段先以静态精选表 + `SlotAllocator` 纯后置投影上线，尚未接入用户自定义「词 = Emoji」映射或按应用自动静默（如终端/IDE），留待后续按需叠加到同一 `SlotAllocator` 接口后方。
  - 其余主候选重排工序（`demote_runner_up_readings`、`normalize_online_slots`）将在 Stage 2/3 统一概率打分器完成后继续收敛进 `SlotAllocator`。
