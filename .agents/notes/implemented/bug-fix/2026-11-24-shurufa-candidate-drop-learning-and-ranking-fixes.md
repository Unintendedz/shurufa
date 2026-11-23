# Agent Note: 修复内联 Emoji 刷新吞词、非连续同音学习失效、语境同音脱节与双字拼凑次选占位

Status: implemented

## Problem

在实机输入「`chufa`」时候选列表中完全找不到「触发」（即使翻遍全部 214 条候选也缺失），而输入「`chufale`」却能找到「触发了」。进一步全链路排查发现，这并非单一词库问题，而是 Stage 1–4 落地后在运行时生命周期、个人学习叠加层、语境同音重排与次选排座四处同时存在结构性缺陷：

1. **P0 内联 Emoji 投影跨键吞词与选词错位（直接诱因）**：`crates/input-runtime/src/runtime.rs::refresh()` 换入引擎新快照时清空了 `self.engine_order`，但漏清 `self.projected_emojis`。输入前缀 `chuf` 时命中「出发」并在下标 `2`、`6` 投影 `🚀`、`✈️`；下一键输入 `a`（`chufa`）后，`order_presentation()` -> `apply_slot_allocator()` -> `strip_projected_emojis()` 用上一键残留的下标 `[2, 6]` 把新快照里的真实中文候选「触发」（下标 `2`）和「处罚决定」（下标 `6`）静默删除。在 51 个常用双字词抽样中，9/51（约 18%）丢失真实候选；若从 `chuf` 退格到无整句重排的单音节 `chu`，`engine_order` 为空时删行不更新映射，还会导致界面显示第 3 项上屏的却是被删掉的前一项（选字错位）。此外 `refresh()` 与 `expand_cached_candidates()` 曾把 `apply_slot_allocator()` 放在 `transform_candidates()` 之前，插件改序或插行也会错位 `projected_emojis` 席位；失焦上屏与向左改字在光标停于投影 Emoji 席位时缺少回退保护。
2. **P0 独立个人学习叠加层在补全词夹层处截断**：Stage 4 的 `crates/engine/src/user_dictionary/overlay.rs::apply_personal_overlay()` 用 `take_while` 仅扫描开头的连续同音词。当精确同音词条数不足首屏（大部分双字/三字词，如 `denglu` 的「登录 / 邓禄 / 登录时间 / 登录名 / 登陆」或 `chufa` 的「出发 / 处罚 / 触发 / … / 除法」）时，长词补全与词网格整句夹在同音词之间，导致夹层后的同音词完全无法被个人学习或手动置顶召回（连续改选 3 次「登陆」或「除法」位次纹丝不动）。
3. **P1 语境同音重排与 Stage 4 个人学习冲突，且未接入 `.gram` 搭配模型**：
   - `crates/engine/src/session/candidates.rs::contextual_dictionary_promote()` 直接拿未改写的静态 `WordItem::weight` 与前文竞争，忽略 Stage 4 的 `personal_selection_events`。结果反复选 3 次弱静态词（如 `chufale` 的「触发了」100 vs「出发了」18,055）后，空上文排第 1，只要前文带任意中性汉字就会被静态权重打回第 2。
   - `pick_dictionary_homophone()` 仅查 4.5MB 字级神经模型，未查询已 mmap 挂载的 `.gram`（如万象语法模型），导致「条件满足后自动 + `chufa`」「超强台风预计明天在沿海 + `denglu`」「水和氧气是人类生存的 + `bixu`」等强搭配短语无法翻转。
4. **P1 开启「去重后补充次选」时双字 `1+1` 拼凑句抢占首屏第 2 格**：当 `sentence_association.show_next_on_duplicate = true` 且双字精确词典词已领先时，词网格补充的 `1+1` 单字拼凑句（如 `chufa` 的「楚伐」、`denglu` 的「邓禄」、`baocun` 的「豹寸」、`sousuo` 的「艘所」）在 `runner_up_order()` 下按 `keep = 1` 留在首屏前排，在 15 个常用双字词抽样中污染了 13 个词的第 2 格，把「登陆」「骑车」「除法」等真正同音词挤出视野。

## Decision

针对四处根因逐一补齐结构性不变量，不引入任何针对特定词条的硬编码特判：

### 1. 统一 `projected_emojis` 生命周期、末步投影顺序与席位不变量守卫

- `crates/input-runtime/src/runtime.rs::refresh()` 在清理 `self.engine_order` 时同步调用 `self.projected_emojis.clear()`（覆盖快照成功与失败两条分支），确保换入引擎新快照时绝不沿用上一键投影下标。
- 在 `strip_projected_emojis()` 入口增加 `debug_assert!` 不变量守卫，断言每条记录均在界内、来源为 `CandidateSource::Emoji` 且 `engine_order[seat] == usize::MAX`，把同类脱节缺陷前移为测试期即刻暴露的断言失败。
- 将 `apply_slot_allocator()` 移出 `order_presentation()`，在 `refresh()`、`expand_cached_candidates()` 与 `finish_settled_presentation()` 三处统一执行「内置排序 -> 插件变换 `transform_candidates()` -> `apply_slot_allocator()`」，并在存在插件插行（`has_inserted_rows()`）时跳过投影，消除插件与 Emoji 席位互相错位的可能。
- `transform.rs::first_engine_seat()` 跳过投影 Emoji 席位；`Action::FinishOnFocusLoss` 与 `Action::Command(Command::ConversionLeft)` 在当前高亮为投影 Emoji 时回退到 `first_engine_seat()`；`snapshot_settled_seats()` 剥离 `usize::MAX` 占位、`settled_highlight()` 排除投影 Emoji、`rerank_settled()`/`apply_settled_rerank()` 在未变序分支恢复投影并按 `keep_highlight_on_same_candidate` 还原高亮；`set_inline_emoji_projection(false -> true)` 在组字中途切换时立即投影并推进代次。

### 2. `apply_personal_overlay()` 跨夹层扫描全部精确同音词典行

- 用 `is_exact_homophone()` 扫描整个候选切片收集全部同码、未纠错、非模糊的精确拼音词典行（无个人事件且无置顶时保持 O(1) 零分配早退），与 `contextual_dictionary_promote()` 的跨补全夹层口径一致。
- 存在夹层时，将新首位、显式置顶、携带个人选择记录（`n_user > 0`）及原本位于连续前段的同音词按后验降序连续排在最前，中间非同音夹层保持原有相对顺序，其余未选过的尾部同音词顺次填回后排空位，使 1 次明确改选即可越过补全词进入第 2 位、3 次改选升至第 1 位且原首位紧随第 2 位。

### 3. 语境同音竞争接入 Stage 4 贝叶斯等效权重与自然对数 1.0× `.gram` 搭配加成

- `crates/engine/src/user_dictionary/overlay.rs` 新增 `effective_bayesian_weights()`：按推导 `alpha * P_static(w) + n_user(w) = (alpha / Z) * (w_static + (Z / alpha) * n_user(w))` 计算等效先验权重（无个人事件时返回 `None`，冷启动行为零变化），由 `session/candidates.rs::contextual_dictionary_promote()` 透传至 `pick_dictionary_homophone()`，防止中性汉字前文把刚学成的常用词打回次席。
- `crates/engine/src/lattice/neural.rs` 新增共享辅助 `homophone_collocation_bonuses()`（全拼与双拼共用），复用已 mmap 的 `collocation_db` 计算跨界搭配加成 `(raw - non_collocation_penalty).max(0.0) * collocation_weight`（要求 `collocation_min_length >= 3` 以避免单字末缀加单字前缀偶然共现），与神经对数概率 `ln P_LM(w | ctx)` 保持同自然对数 1.0× 量纲，不再乘额外经验折减系数。

### 4. `runner_up_order()` 仅让双字精确词典领先时的 `1+1` 拼凑句退出首屏

- 词网格 `decode.rs` 中任意双字整词边（含指物代词「它」双证据合成边，见 [指物代词多样性与排座](2026-11-14-lattice-pronoun-diversity-and-seating.md)）携带 `PHRASE_LENGTH_BONUS (20)`，每字静态证据恒 `>= -13.0 nats/字`；而 `1+1` 单字拼凑边缺少词长加成且两边各扣 `ln(1e6)`，每字静态证据恒 `<= -16.4 nats/字`。
- 在 `crates/engine/src/ordering.rs::runner_up_order()` 中，当 `width == 2 && first_reading > 0` 且第 0 位为未纠错词典命中时，将每字静态证据 `< -14.5 nats/字`（`TWO_CHAR_SPLIT_FALLBACK_MAX_MEAN_NATS`）的 `1+1` 拼凑句退居首页前排（总预算 6 格 `TWO_CHAR_DEFERRED_PAGE_BUDGET`）及剩余精确词典同音词（如「除法」）之后、普通单字之前；保留二次调用零分配恒等性、保留「它在」等双字合成边在首屏，也不影响未登录双字短语（`first_reading == 0`）。

## Historical audit

- [Shurufa 槽位分配器与精选内联 Emoji 投影](../feature/2026-11-20-shurufa-slot-allocator-and-inline-emoji.md)：部分重叠，本篇修复其 `Runtime::refresh()` 遗漏清理 `projected_emojis`、与插件变换的先后顺序及失焦/改字边界问题。
- [Stage 3 单遍 4.5MB 神经复用、停顿首选锁定与语境同音标定](../architecture/2026-11-20-stage3-single-pass-scorer-and-settled-lock.md)：部分重叠，本篇在保持停顿首位零翻转与单遍缓存前提下，将 `.gram` 跨界搭配与 Stage 4 等效先验接入 `pick_dictionary_homophone()`。
- [Stage 4 独立个人学习叠加层](../architecture/2026-11-20-stage4-decoupled-personal-learning-overlay.md)：部分重叠，本篇修复其 `apply_personal_overlay()` 遇到补全词夹层提前截断的问题，并打通叠加层与语境同音竞争的衔接。
- [离合词解码与次选整句静默让位](2026-11-18-separable-vo-compounds-and-runner-up-gating.md)：部分重叠，本篇把基于 `StaticEvidence::mean()` 的静默让位策略延伸到双字精确词典领先时的 `1+1` 拼凑句。
- [指物代词多样性与排座](2026-11-14-lattice-pronoun-diversity-and-seating.md)：沿用其双证据双字合成边分值区间，未改动其解码行为。

## Alternatives considered

### 为 `chufa` 等问题词直接调高静态词频或关闭内联 Emoji /「去重后补充次选」出厂开关
最强理由是改动极小、能迅速掩盖表面症状。未采用，因为「触发」消失的根因是 `refresh()` 漏清 `projected_emojis` 造成的通用跨键吞词与选词错位（抽样命中率 ~18%），调词频完全掩盖不了退格错位与其他词吞词；直接关闭用户已开启的 Emoji 或次选开关既越权改变产品配置，也无法解决开着开关时的真实缺陷。

### 在 `pick_dictionary_homophone()` 中对 `.gram` 搭配加成乘以 `0.5` 保守折减
最强理由是降低 `.gram` 噪声语料对神经模型判断的干扰。未采用，经 `convert_eval` 扫参实测，`0.5×` 在 `shurufa-spoken-v1` 的 `context-homophone` 上 `top1` 仅维持 `0.706`（12/17）、`chufa` 语境 6 例仅 `0.333`（2/6）；改为与神经对数概率同域的自然对数 `1.0×` 后，`context-homophone` 升至 **`0.765`**（13/17，`mrr 0.816 -> 0.873`），`shurufa-spoken-v1` 整体 `top1` 升至 **`0.808`**、`mrr` 升至 **`0.876`**，`chufa` 语境 6 例升至 **`0.833`**（5/6），且长句与大词表零回归。

### 在引擎词网格阶段直接过滤双字 `1+1` 拼凑句而非在 `runner_up_order()` 降位
最强理由是省去多余的词网格候选行。未采用，因为过滤会破坏「挪而不删」原则——用户遇到词库未收录的双字人名/新词（例如有同音词典词但仍想打 `1+1` 组合）时将完全翻不到该候选；在 `runner_up_order()` 中让位至第 2 页开头，既清空首屏杂音又保留第 2 页完整可达性。

## Consequences

- **收益**：
  - 彻底根除内联 Emoji 投影导致的跨键吞词与退格选词错位（51 词实机探针差异从 `9/51` 降至 `0/51`，`chuf -> Backspace -> chu` 选字与显示 100% 一致）。
  - 非连续同音词（`denglu` 的「登陆」、`chufa` 的「除法」等）在 Stage 4 个人学习与手动置顶下恢复正常召回与首位竞争，且中性前文不再击退已学成的弱静态首位。
  - 完整 `convert_eval` 对照（万象 `.gram` + 4.5MB + 25MB + 纠错 + `--settled-lock-leader`，页宽 6）：
    - `shurufa-spoken-v1` (n=120)：`top1 0.800 -> 0.808 (+0.008)`，`mrr 0.868 -> 0.876 (+0.008)`，`context-homophone top1 0.706 -> 0.765 (+0.059)`、`mrr 0.816 -> 0.873 (+0.057)`，`flips = 0`。
    - `sentences-v2` (n=312)：`top1 0.740`、`top5 0.933`、`mrr 0.832`、`flips = 0`（零回退）。
    - `sentences-neutral-v1` (n=139)：`top1 0.784`、`top5 0.957`、`mrr 0.868`、`flips = 0`（零回退）。
    - `quanpin-words-v1` (n=2,791)：`top1 0.755`、`top5 0.967`、`top6 0.978`、`found 0.989`、`flips = 0`（仅 `top9` 因 1 例 `1+1` 退至第 2 页微调 `0.985 -> 0.984`）。
- **代价与后续重访条件**：
  - `homophone_collocation_bonuses()` 依赖万象 `.gram` 的共现统计，在少数网络语料高频偏误搭配（如「自己制定」「频繁反应」）上仍会让 `.gram` 抵消微弱神经优势（`sp-061`、`sp-068`）；若后续更换或重训更干净的 `.gram` 或更大容量神经键盘模型，应重新在 `shurufa-spoken-v1` 上复验 `collocation_min_length` 与同音竞争权重。
  - `TWO_CHAR_SPLIT_FALLBACK_MAX_MEAN_NATS (-14.5)` 依赖 `decode.rs` 当前 `PHRASE_LENGTH_BONUS (20)` 与单字退避扣分的静态证据刻度；若未来重写词网格静态证据标度，需同步核对该阈值并重跑 `runner_up_order` 单测。
