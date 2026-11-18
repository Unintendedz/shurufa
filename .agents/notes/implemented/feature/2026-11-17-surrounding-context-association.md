# Agent Note: 组字开始前同步光标前现成文本并按前文重排首组字典候选

Status: implemented

## Problem

在已有文字的输入框中接着打字、粘贴一段文字后继续输入、或把光标点回句子中间补词时，候选排序与联想完全感知不到输入框里的现成前文：

1. **宿主未同步光标前现成文本**：macOS 宿主仅通过 `selectedRange` 与 `attributedSubstringFromRange:` 读取紧邻光标的 1 个字符用于智能标点判断；`Runtime::ai_context` 与 `Engine::rescoring_context` 只累计当前焦点会话里由输入法自己刚刚上屏的文字，切窗、挪光标或面对输入框原有文本时前文为空或停留在旧光标位置。
2. **首组同音字典词不吃前文**：即便前文进入 `rescoring_context`，引擎内的逐键神经模型和运行时的停顿精排都只重排多词整句路径（`Generated`、`NeuralKeyboard`、`Collocation`），词典直接命中的同音词（如「美好的」后敲 `huiyi` 选「回忆」与「会议」、「竭尽」后敲 `quanli` 选「全力」与「权利」、「把理论付诸」后敲 `shijian` 选「实践」与「时间」）完全停留在静态词频次序。

同时，桌面端在空闲未敲拼音的状态下不应主动弹出任何下一词预测窗遮挡编辑视线。

## Decision

从 macOS 宿主、Host API、输入运行时与 Rust 引擎四层打通仅在组字期间生效的光标前现成文本联想：

1. **空闲态零打扰与首键单次读取（`shared/apple/TextClient.*`、`AXPrecedingContext.h`、`InputController.mm`）**：
   - 空闲不敲拼音时不读取文档、不触发预测、不弹出候选窗。
   - 仅当会话处于空闲态、方案为全拼/双拼/五笔、非英文模式、非安全输入（`secureEventInputActive`）且非终端应用时，在按下首枚 ASCII 字母键进入组字前读取一次光标前至多 96 个 UTF-16 码元的现成文本；同一轮组字后续按键不再跨进程回读文档。
   - 优先经 `IMKTextInput`（`selectedRange` + `attributedSubstringFromRange:`）读取并剥除截断边界上的孤立 UTF-16 代理项；以弱引用记录已证实可读的客户端，使光标移至非空文档 0 位或删空已知可读文档时准确同步空串 `@""`，而始终返回 `nil` 的终端类客户端保持未知状态。
   - 当客户端未通过 `IMKTextInput` 暴露文本（如部分 Electron / Chromium 输入框）且进程已获辅助功能信任时，以 20 毫秒超时查询焦点元素的 `kAXSelectedTextRangeAttribute` 与 `kAXStringForRangeParameterizedAttribute`（回退 `kAXValueAttribute`），拒绝 `AXSecureTextField` 子角色，且从不弹窗请求权限；若仍不可读则不覆盖运行时已有的本会话上屏历史。
2. **运行时前文清洗、连续后缀保持与外部跳变断链（`Runtime::set_surrounding_context` + `msime_client_set_surrounding_context`）**：
   - 新增 C 接口 `msime_client_set_surrounding_context`（上限 4096 字节 UTF-8）：隐私会话或带 `TRAIT_SECURE` / `TRAIT_PRIVATE` 字段属性时清空上下文并返回 `false`；组字进行中返回 `false` 不改状态；空闲合法调用同步至 `Runtime::set_surrounding_context`。
   - `Runtime::set_surrounding_context` 剔除富文本附件占位符 `U+FFFC` 及在线查询拒收的控制字符，按字符边界保留末尾 1024 字节；若非空且当前 `ai_context` 已以该段文本结尾，保留完整 `ai_context` 与引擎 `CommitChain` 不变；否则调用 `engine.reset_context()` 切断旧词序链并更新 `ai_context` 与 `engine.set_rescoring_context`。
3. **引擎前段多字同音系统词典词按汉字前文提首（`NeuralReranker::pick_dictionary_homophone` + `InputSession::contextual_dictionary_promote`）**：
   - `NeuralReranker` 为同音词典评分独立持有一份共享 `Arc<SentenceModel>` 只读权重的 `Reranker` 状态，使其上文 KV 缓存与整句路径跨按键续算的 `TailRun` 互不覆盖。
   - 当 `rescoring_context` 最近 64 字窗口内含有汉字且末尾连续汉字子句（由 `text::trailing_han_clause` 剥离表情符号、空格、标点与 ASCII 前缀，见[剥离非汉字上文 `<unk>` 污染并向词格传入连续汉字前文](../bug-fix/2026-11-18-han-clause-context-sanitization.md)）非空、处于无局部模式、无光标前缀截取、无激活辅助码的完整音节（`is_intact`）拼音组字、个人词序链（`personal_context_rerank_order`）未命中转移、且首候选为字数 `>= 2`、无模糊音与纠错偏移、回答完整按键（`selection_completes_composition` 且 `reading_answers_key`）、未经手动置顶覆盖的 `CandidateSource::Database` 行时（见[超高频核心词调频护城河与语境同音解耦](../bug-fix/2026-11-18-core-word-frequency-moat.md)），在前 32 行候选内跳过夹杂的更长前缀补全词（如「登录」与「登陆」之间的「登录时间」），收集至多 12 条与首行读音和字数完全一致的系统词典同音词交由逐键神经模型评分。
   - 仅当最佳同音词相对原首选满足 `(ln(weight) + neural_nats) - leader >= 1.5` nats（等权混合优势不少于 `0.75` nat）时将其单独旋至第 0 位，其余次选保持词典原序不重排；重排前列表存入 `ranking_candidates`，不改变后续 `apply_candidate_positions` 固定排位与选词调频的基准顺序；半截声母键、无前文或纯 ASCII 前文时零开销跳过。

## Historical audit

- [上屏后联想与组字状态分离](2026-10-24-post-commit-prediction.md)：职责正交。该篇管 Android 空闲态的 `next_word_prediction`；macOS 与 Shurufa 保持 `next_word_prediction` 关闭，空闲时不展示任何建议。
- [Shurufa 随包提供停顿后精排模型](2026-11-08-shurufa-settled-rerank.md) 与 [九键遵守句子联想设置](2026-10-09-nine-key-sentence-options.md)：直接受益。`set_surrounding_context` 同步更新 `ai_context` 与 `rescoring_context`，4.5 MB 逐键模型、25 MB 停顿精排模型与 AI 候选同时读到输入框现成前文。
- [独立句子排序保留词网格的静态证据](../bug-fix/2026-10-14-rerank-static-evidence.md)：保留 `ordering.rs` 对无词频权重的外部 `DICTIONARY_SOURCES` 首选豁免契约不变；字典同音重排放在拥有真实 `weight`、置顶表与个人调频状态的 `crates/engine` 会话层内执行。
- [Linux surrounding text 不可用时保留智能标点前文](../bug-fix/2026-10-17-linux-smart-punctuation-shadow.md) 与 [脚本插件命令候选行](../architecture/2026-10-13-script-plugin-command-rows.md)：保留智能标点独立影子与「插件行不进 `ai_context`」的不变量。

## Alternatives considered

- **对单字、不同读音简拼或减去空上文 `<bos>` 概率做全组字典重排**：`SentenceModel` 按 `<bos> + context + candidate` 做字级自回归计分，单字缺乏词内搭配约束，实测会把低频实词字翻到高频代词前面；减去空上文 `ln P(w | <bos>)` 则会奖励在句首极少出现而在逗号后常见的生僻字（如把 `ta` 翻成「塌」）；全组排序还会在模型置信度不足时让「娇艳」等强字内转移词挤掉「校验」等次选。限定为完整音节、字数 `>= 2`、严格同音且仅在超过 `1.5` nats 阈值时把第 1 名旋到首位，可同时消除这三类退化。
- **在 `ordering.rs` 里取消 `DICTIONARY_SOURCES` 的可信词典豁免**：能同时让外层 25 MB 停顿模型重排词典行，但外层 `OrderRow` 不携带词典 `weight`、置顶标记与用户调频状态，直接放开会在无静态先验约束下把低频同音字翻到首位，也会破坏既有的零分配拒绝门禁。
- **在每次焦点进入、鼠标点击或每次按键时都回读客户端文档**：能让空闲态随时持有最新前文，但 `IMKTextInput` 每次调用都是跨进程同步 XPC 往返，在窗口切换时易触发 Chrome 等客户端的激活互锁卡顿（见 `TextClient.h` 注释）；收敛到空闲首枚字母键单次读取即可覆盖粘贴、删改和光标跳转，且零空闲开销。
- **在空闲态放开 `next_word_prediction` 主动弹下一词**：能在不敲键时给出续词，但在桌面编辑器和聊天框中常驻候选浮窗会遮挡正文并干扰方向键/空格预期，用户明确要求未敲拼音时不弹出任何内容。

## Consequences

- **收益**：在原生输入框及支持辅助功能读取的 Electron / 浏览器输入框里，无论是接在现成句子后面敲整句还是敲单个同音词、双字词、简拼词，候选都能结合光标前已有文字排序，挪动光标或清空文本后旧上文立即失效，空闲态无弹窗、无额外跨进程读取。
- **代价与边界**：开启逐键神经模型且光标前存在汉字时，首候选为系统词典行的组字每次按键多一次至多 12 条短词的神经评分（共享 64 字前缀缓存，亚毫秒级）；完全关闭 `neural_keyboard` 时字典同音词仍保持静态词频顺序（整句仍可由停顿精排或 AI 候选利用前文）；终端应用与未开启辅助功能授权且不回传 `IMKTextInput` 文档的应用无法读取现成文本，继续回退到本会话输入法自己上屏的历史。
