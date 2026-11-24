# Agent Note: 修复 Stage 4 单字首位劫持、空格选择偏差与单字语境同音重排缺失

Status: implemented

## Problem

在输入框现成文本「`邮箱|的邮件要删掉。`」中敲 `li`，候选栏首位出现「李」而非「里」（`1 李 2 里 3 力 4 理 5 离 6 立`）。实测排查定位到三个相互叠加的结构性根因：

1. **Stage 4 个人学习叠加层存在 600 字长尾稀释与空格选择偏差，导致 20+ 核心单字首位被全面劫持**：
   - `crates/engine/src/user_dictionary/overlay.rs::apply_personal_overlay()` 原先以全组同音词权重之和 `total_static_mass` 归一化 `P_static(w)`。在单字键（如 `li` 有 601 个同音字、`shi` 有 398 个、`you` 有 284 个）及多同音词键（如 `chu'fa` 有 4 个高频同音词）下，静态榜首占比被海量长尾稀释到 `0.15–0.34`，等效先验 `3.0 * P_static` 仅剩 `0.45–1.02`。
   - 同时 `crates/engine/src/session/learning.rs::learn_ranked_candidate()` 仅记录非第 0 位的改选、对空格接受默认首位完全不计数，形成严重观测选择偏差：用户一个月里用空格输入 100 次「里」「的」「是」「好」「在」计 `0`，仅偶发选 2 次人名「李」或日期「号」或补语「得」就满足 `n_user >= 1.8` 并直接反超，导致「里→李」「的→得」「是→事」「有→又」「在→再」「好→号」「吗→嘛」「大→打」「等→蹬」「他→它」等 20 个高频核心单字首位全面沦陷；甚至在「李」劫持后用户主动改选一次「里」，`1.00 + 1.0 < 0.20 + 2.0` 依然抢不回首位。
2. **单字语境同音重排完全关闭**：`crates/engine/src/session/candidates.rs::contextual_dictionary_promote()` 硬性写死 `if characters < 2 { return None; }`，即使光标前已有「邮箱」「我同事老」「这句话说」「今天是几」「这个事情我们明天」等强前文且挂载了 4.5MB 键盘神经模型与万象 `.gram`，单字输入完全不参与语境消歧。
3. **macOS `MSIMETextClientPrecedingContext` 对非起点空串返回误判为可读空文档**：`shared/apple/TextClient.mm` 在 `selected.location > 0`（请求 `length >= 1` 的前文切片）且客户端返回非空对象但 `.length == 0` 的空串（部分 Electron / Chromium 控件行为）时，原先标记 `provedReadable = YES` 并返回 `@""`，直接跳过 `ReadAXPrecedingContext` 兜底且把运行时已有的前文清成空串。

## Decision

### 1. 修正 Stage 4 先验锚点、单字第 1 槽保护与多音节竞争键空格确认

- **单字键未置顶永不劫持第 1 槽**（`crates/engine/src/user_dictionary/overlay.rs::apply_personal_overlay`）：对单音节单字键（`!entry_key.contains('\'')` 且首行 1 字），未手动置顶的 `personal_selection_events` 只对第 2 位及以后的次选排序（保证近期选过的「李」「号」「得」立即召回到首屏第 2 位），绝不替换静态第 1 槽；单字第 1 槽只受显式手动置顶（`pinned_candidates`，L4 硬约束）或语境同音重排控制。同时单字键的 `effective_bayesian_weights()` 返回 `None`，避免历史单字改选扭曲语境消歧。
- **静态先验改以榜首 `w_max` 为锚**：将 `apply_personal_overlay()` 与 `effective_bayesian_weights()` 的静态先验由 `alpha * (w / sum(w))` 改为 `alpha * (w / w_max)`，确保无论同音组有 2 个还是 20 个词，静态榜首始终保有完整 `alpha = 3.0` 护城河。
- **竞争多音节键空格确认（零闲时开销）**：新增 `record_contested_leader_confirmation()` 并在 `session/learning.rs::learn_ranked_candidate()` 的 `index == 0` 路径下调用——仅当多音节键在 `personal_selection_events` 中已存在竞争词且当前首选计数尚未领先最高竞争词 `2.0` 次时才记入一条确认事件（无竞争键零写入，领先后停止追加），消除长期使用中「空格不计、偶发改选无限累积」的选择偏差。

### 2. 放开单字语境同音重排并分档标定阈值

- `crates/engine/src/session/candidates.rs::contextual_dictionary_promote()` 将字数门槛由 `characters < 2` 放宽至 `characters == 0`（仍严格要求完整音节 `is_intact`、精确同音 `Database` 命中、前 12 条高频候选且尊重手动置顶）。
- `crates/engine/src/lattice/neural.rs::homophone_collocation_bonuses()` 按 `min_len = if chars <= 1 { 2 } else { 3 }` 统一跨界搭配最小长度（至少覆盖 1 个前文汉字 + 完整单字，或多字词 3 字跨界），使「邮箱里」「说得」「老李」「一件」均可命中 `.gram`。
- `pick_dictionary_homophone()` 对单字采用独立标定参数：`SINGLE_CHAR_STATIC_PRIOR_WEIGHT = 0.40`、`SINGLE_CHAR_PROMOTE_MARGIN_NATS = 1.00`（多字词维持 `0.25 / 0.35` 不变），在明确强语境精准翻盘的同时守住中性前文不误翻。

### 3. 修复 macOS `TextClient.mm` 非起点空切片误判

- `shared/apple/TextClient.mm::MSIMETextClientPrecedingContext()` 在 `selected.location > 0`（`length >= 1`）但 `attributedSubstringFromRange:` 返回 `nil` 或空串时统一返回 `nil` 且不置 `provedReadable`，让 `InputController.mm` 正常回退 `ReadAXPrecedingContext` 且不覆写运行时前文。

## Historical audit

- [Stage 4 独立个人学习叠加层与确定性退格回滚](../architecture/2026-11-20-stage4-decoupled-personal-learning-overlay.md)：部分重叠，本篇修正其全组求和归一化稀释、单字未置顶抢占第 1 槽及空格不计数的选择偏差。
- [组字开始前同步光标前现成文本并按前文重排首组字典候选](../feature/2026-11-17-surrounding-context-association.md)：部分重叠，该篇在尚未引入万象 `.gram` 与分档静态防噪系数前曾排除单字语境重排；本篇在引入 `.gram` + `0.40 / 1.00 nats` 单字护栏后将其安全放开，并修复了 `TextClient.mm` 对非起点空串的误判。
- [修复内联 Emoji 刷新吞词、非连续同音学习失效、语境同音脱节与双字拼凑次选占位](2026-11-24-shurufa-candidate-drop-learning-and-ranking-fixes.md)：延续同批候选质量根因修复，共用 `homophone_collocation_bonuses()` 与 `effective_bayesian_weights()`。

## Alternatives considered

### 仅靠提高 `BAYESIAN_PRIOR_ALPHA` 或单字 `SLOT_ZERO_MIN_DECAYED_PICKS` 而不禁止单字劫持第 1 槽
最强理由是公式统一、无需区分单字与多音节词。未采用，因为主词库里大量高频单字处于同档近等权（「是/事/时」均 `~31.4M`、「在/再」均 `~19.2M`、「有/又」均 `~17.8M`、「一/以」均 `~35.2M`），只要单字允许未置顶抢占第 1 槽，在用户实际反复输入日期「号」8 次、补语「得」4 次、指物「它」22 次后仍必然把「好」「的」「他」挤下首位；按单字仅做首屏第 2–5 位召回、首位交由语境与手动置顶负责，语义与肌肉记忆更清晰。

### 单字语境同音竞争直接复用多字词的 `0.25 / 0.35` 阈值（或 `0.25 / 0.60`）
最强理由是少维护一组常量。未采用，经 60 条单字语境评测集实测，`0.25 / 0.60` 在 25 条翻盘用例上与 `0.40 / 1.00` 持平（均为 `15/25`），但在中性前文（如「我刚才问 + ta」）上更容易把低频/次选单字翻到首位；`0.40 / 1.00` 在保证明确强语境稳定翻盘的前提下提供更高安全边际。

## Consequences

- **收益**：
  - 无需清空或修改用户现有 `msime_user.db`，此前被劫持的 20 个高频核心单字（`li→里`、`de→的`、`shi→是`、`zai→在`、`hao→好`、`you→有`、`ta→他`、`ma→吗`、`da→大`、`deng→等`、`ba→吧` 等）全部即时恢复第 1 槽，近期选过的同音字仍保留在第 2 位首屏召回。
  - 60 条合成单字语境评测集（万象 `.gram` + 4.5MB + 25MB，页宽 6）：整体 `top1 = 0.817`、`top5 = 1.000`、`mrr = 0.893`（其中 `ctx-flip = 15/25`，覆盖「邮箱里」「我同事老李」「这句话说得」「今天是几号」「明天再」「我买了一件」等；`ctx-keep = 14/14`；`neutral-keep = 20/21`）。
  - 原有全集零回退：`shurufa-spoken-v1` 维持 `top1 = 0.808`、`mrr = 0.876`；`quanpin-words-v1` (n=2,791) 维持 `top1 = 0.755`、`top5 = 0.967`。
  - `TextClientTest.mm`、Rust 全量单测（`msime-engine`、`msime-input-runtime` 411/411、`msime-engine-wasm` 61/61、`msime-host-api shurufa_tests` 6/6）全绿。
- **代价与后续重访条件**：
  - 若未来新增**光标右侧上下文**（如句中补字 `邮箱[li|]的邮件` 的后文 `的邮件`）跨 FFI 传入，单字和词组消歧（尤其是「慢慢地/的」「写得/的」）还可进一步提升；当前仍仅利用光标左侧上下文。
  - 若用户确实希望某个单字在无上下文时永久取代静态第 1 槽（例如长期偏好「她」或「把」），需使用手动置顶，不再靠隐式累计自动抢占。
