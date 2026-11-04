# Agent Note: 全拼、双拼的 Tab 笔画筛选

Status: implemented

## Problem

维护者问 Shurufa 有没有搜狗拼音的「Tab 加笔画」：打完拼音按 Tab，再用 `h` `s` `p` `n` `z`（横竖撇点折）按笔顺筛候选，用来在同音字里快速找到要的那个字。之前只有移动端九宫格有笔画筛选（[九宫格展开面板](2026-10-08-nine-key-expanded-panel.md) 的 `NineKeySession::set_filter`），实体键盘的全拼、双拼没有，Shurufa 的包里也不带 `msime-stroke.db`。

维护者翻页只用 `-=`，选词用 `[]`，Tab 在 Shurufa 上原本只是固定开着的翻页键，平时用不到；双拼用得少；Windows、Linux 这次不跟。

## Decision

- 筛选是输入算法，放在 Engine 的会话里（`crates/engine/src/session/stroke_filter.rs`），宿主只负责把 Tab 交给会话。
- 进入：`SessionOptions::tab_stroke_filter` 开着、`stroke_dictionary` 不为空，全拼或双拼在组字、光标在末尾，不在本地模式、专用英文、整句改字、光标后编辑和显式辅助码前导里，这时快照的 `spelling_symbols` 带 `\t`。macOS 宿主原本就按 `spelling_symbols` 里有没有 `\t` 决定 Tab 交不交给会话（双拼手动辅助码用的同一条路），所以宿主不用改按键路由。双拼开着 Tab 手动辅助码时 Tab 归辅助码。笔画词库在第一次按 Tab 时才打开，打不开时报 `LANGUAGE_DICTIONARY_UNAVAILABLE`，组字不变。
- 筛选里：`hspnz` 追加一笔，`x` 是任意一笔；其他字母和 Tab 被吞掉；Backspace 删一笔，删光后再按一次退出，再按才删拼音；数字、空格、标点、翻页照常。移动光标和向后删先退出筛选再照常处理；选中一行（`commit`）、Cancel、`reset_composition` 都结束筛选。
- 笔画不进组字原文。原文、光标、`editing_text` 都不变，只在快照的 `preedit` 后面接 ` 笔：一丨`，`MSIMEPreeditCaretPosition` 认不出这串字母时本来就把光标放在末尾。筛选记下按 Tab 时的原文，`update_mixed_candidates` 每次刷新都核对一遍：原文、光标、模式任何一项变了就结束筛选，不靠每条修改路径各自记得清它。
- 比的是候选的第一个字：`stroke::characters_with_stroke_prefix` 从 `msime-stroke.db` 取出笔顺以这几笔开头的全部单字，词和整句看它的第一个字，英文和符号行被筛掉。九宫格的笔画筛选改用同一个函数。带 `x` 时走新加的 `LanguageDictionary::texts_matching_key_prefix`（GLOB，字面前缀限定扫描范围），不带时就是原来的 `texts_with_key_prefix`。
- 一笔筛不出任何候选时不收这一笔，候选窗不会被筛空。
- 进入筛选时先 `expand_initial_candidates`：单字母首段只给前几十个单字，筛选要在全部单字里找。
- 筛掉的行留在 `ranking_candidates`，学习、固定位置经 `ranking_index` 按原列表里的名次找行：靠笔画从后面翻出来的字照常往前调。

### 偏好、版本表和打包

- 共享偏好 `tab_stroke_filter`，默认开（维护者拍板），开着时不写进文档。只在 Shurufa 的 macOS 原生设置页「输入方案 → 全拼选项」里给开关，React 设置页不列（`scripts/test-preferences-field-parity.py` 的 `RUST_ONLY` 记了原因）。
- host-api 只在 macOS、而且版本表 `features.tab_stroke_filter` 为真时把偏好交给 Engine（`tab_stroke_filter_applies`）。
- 版本表新增 `features.tab_stroke_filter`：full（「全部功能」规则）和 shurufa 为真。为真的版本必须在 `language_dictionaries` 里列出 `msime-stroke.db`、必须有全拼或双拼，`scripts/test-editions.py` 检查。Shurufa 因此列出 `msime-stroke.db`。
- `build-shurufa.sh` 先跑 `scripts/fetch_language_dictionaries.py`（按锁下载并校验，已有则不动）；`stage-resources.sh` 对 full 以外的版本只暂存版本表列出的语言词库；CMake 把 `target/macos/language-dictionaries` 拷进 Shurufa 的 `Contents/Resources/language-dictionaries`，缺 `msime-stroke.db` 或它的许可证时配置失败。`prepare_host` 每次启动都在 `EngineResources` 旁边找这个目录，写进 `language_dictionaries`。

## Alternatives considered

- **把笔画写进组字原文，当成一种辅助码让 provider 处理**（双拼手动辅助码的做法）：笔画能跟着原文编辑、撤销。但 `hspnz` 本身是拼音字母，原文里混进它们，分词、纠错、学习的读音、Enter 上屏的原文都要认一个新标记；而筛选只关心「第一个字的笔顺」，在会话最后一步收窄列表就够了，provider 一行不用动。
- **筛选在宿主做**：宿主手里有候选，看起来不必经过 Engine。但笔画词库只有 Engine 读得到，Engine 只交出前几页候选，单字大多在后面；九宫格当初同样因为这两点放在引擎里。
- **比较候选的每一个字**：对词更严格，但搜狗和九宫格都按首字，用户心里想的是「这个字怎么写」。
- **筛空就显示空列表**：照实反映结果，但空列表下空格会上屏拼音原文，多半是笔顺记错了，不收这一笔更不容易出错。
- **偏好默认关**：不改 Tab 原来的手感。维护者不用 Tab 翻页，明确要求默认开；full 的 macOS 宿主同样带这项功能，只是要下载过笔画词库才生效。
- **只用 `edition.id == "shurufa"` 判定、不加版本表功能位**：改动少。但 Shurufa 列出 `msime-stroke.db` 需要一个能被 `test-editions.py` 检查的理由，「不需要的不能列」这条规则要么放宽要么有据可依，功能位就是那个依据。

## Consequences

- Shurufa 的包多了 2.3 MB 的 `msime-stroke.db` 和它的 LGPL-3.0 许可证文本（`rime-stroke-LGPL-3.0.txt` 原本就随包）；第一次构建要联网下载语言词库（三套一起下，约 24 MB，只暂存笔画那套）。
- 全拼、双拼组字时 Tab 不再翻页、不再打开展开候选，而是进入笔画筛选；关掉偏好就恢复原来的行为。
- 预编辑带上笔画后不再是纯拼音，macOS 按预编辑存的本机候选置顶（`MSIMECandidatePinCode`）在筛选期间对不上原来的拼音，筛选里看不到那份置顶，退出后恢复。
- Windows、Linux 的 host-api 不把偏好交给 Engine，行为不变。
- 测试：`crates/engine/src/session/tests/stroke_filter.rs`（进入、筛选、不收空筛、退格、选词、光标、双拼两种情形、学习名次、词库打不开），`language_dictionary` 的通配前缀查询，host-api 的偏好到视图，client-core 的偏好默认值与版本表功能位。
