# Agent Note: Shurufa 随包提供停顿后精排模型

Status: implemented

## Problem

完整版的「桌面神经联想」开关写 `sentence_association.neural_desktop`，打开时由设置应用按需下载 25 MB 的落定重排模型（`resources/settled-model.lock.json`），停止输入后由 `msime_client_start_settled_rerank` 在后台再排一次整句。Shurufa 没有那个设置应用，也没有资源包下载入口；它的原生设置页把同一个开关改名「本地神经排序」，改成写 `neural_keyboard`（每次按键都跑的 4.5 MB 模型）。结果是 Shurufa 用户没有任何办法用上 25 MB 那个模型，而模型仓库的实测说它在原先排错的整句（专门收集的一批）上首选正确率从 0.123 提到 0.613。

## Decision

- 模型随包：`build-shurufa.sh` 按锁文件下载（`scripts/fetch_settled_model.py`，校验 SHA-256 和大小），`stage-resources.sh` 暂存到 `target/macos/settled-model`，CMake 在 Shurufa 版本把它放进 `Contents/Resources/settled-model/`，缺了就拒绝配置。host-api 的 `settled_model_beside` 本来就在 `EngineResources` 旁边找这个目录，宿主的停顿计时和轮询（`InputController` 的 `scheduleSettledRerank`）对所有版本都已经接好，不用改。`verify_shurufa_bundle.py` 按锁文件核对随包的那份。
- 开关：Shurufa 的「词库与数据 → 云端与智能候选」多一行「停顿后精排」，写 `neural_desktop`，defaults 键 `MSIMEClientSentenceAssociationSettledRerank`，照 `show_next_on_duplicate` 的平铺键模式接进探针、覆盖表、合并和套用共享文档。其他版本不显示这一行，读值恒为关，合并时也不写，那里原来的开关已经管着 `neural_desktop`。「本地神经排序」仍然只管 `neural_keyboard`，两个开关互不影响。
- 出厂值不变：版本表里 Shurufa 的 `neural_desktop` 仍是关（`preferences.rs` 的 Shurufa 默认值），由用户自己打开。

## Alternatives considered

- **把「本地神经排序」改回写 `neural_desktop`，与完整版一致**：只有一个开关，少一行设置。但 4.5 MB 模型在每次按键时排序，25 MB 模型只在停顿后接手，两者是不同的取舍：只开大模型时，打字过程中显示的是没重排过的词网格结果，停下来才变。合成一个开关会让已经开着小模型的用户悄悄换成另一种行为。
- **像完整版一样按需下载**：包小 25 MB。但 Shurufa 没有承载下载、进度和失败重试的设置应用，要在输入法进程里补一套下载界面；随包只多一个文件，构建时按同一把锁校验。

## Consequences

- Shurufa.app 大 25 MB；开关打开时，每次输入停顿后台多一次评分（模型仓库给出的单次 p95 约 153 ms），不阻塞按键。建会话时只要装了模型就加载它（`ffi/session.rs`，开关关着也一样，权重以 int8 常驻约 25 MB），`set_settled_rerank_enabled` 只决定用不用它；这是 host-api 现有的行为，这里不改。
- 本机用当前词库对 10 个合成句子比较过四种组合（都不开、只开 4.5 MB、只开 25 MB、两个都开）：25 MB 只在已经出现的候选之间重排，10 句里改动了 2 句的首选，一句变好（「数据库连得」）一句变差（「太懒了」）。两个都开时只改了 1 句，而且那句改前改后都不对。这个样本量分不出两个模型的好坏，只说明它确实接上了、行为与设计一致。

## Verification

- `bash platforms/macos/build-shurufa.sh` 下载并随包放入模型；`python3 platforms/macos/scripts/verify_shurufa_bundle.py target/macos-isolated/Shurufa.app` 核对大小和哈希。
- `ctest -R "preference|settings|shurufa|cloud-appearance|desktop-settings"` 31 项通过，其中 `shurufa-settings-brand` 逐页遍历 Shurufa 的可见设置页。
- 上面的四种组合比较用临时的 host-api 测试经 C 入口建会话、敲键、调 `msime_client_rerank_settled`，数据是本机用户目录的副本，测试没有提交。

## Related

- [原生设置补开关](2026-10-21-native-settings-missing-switches.md)：`neural_desktop` 与 `show_next_on_duplicate` 的平铺键模式出自那里。
- [Shurufa 原生界面使用独立产品身份](2026-10-28-shurufa-product-brand.md)：Shurufa 设置页的可见页和品牌审计。
