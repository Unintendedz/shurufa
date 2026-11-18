# Agent Note: Shurufa 原生接入万象 LTS 语法搭配模型

Status: implemented

## Problem

引擎层和 `msime-host-api` 早已支持通过 `sentence_association.grammar_model` 加载 Rime `.gram` octagram 语法搭配模型（`GramDb`），且 `msime-client-providers` 也内置了 `grammar-model-wanxiang` 资源包的下载与本地快照机制，但 `Shurufa` 独立版本此前未接通这条链路：

1. macOS 原生设置页（`AppearancePreferences.mm`）在 `Shurufa` 品牌分支下隐藏了自定义语法模型弹窗，仅暴露 4.5 MB 本地神经排序和 25 MB 停顿后精排，没有「万象语法搭配」开关，`preference_coverage.py` 只靠探针兜住共享字段。
2. 跨平台共享偏好（`crates/client-core/src/preferences.rs`）对 `sentence_association.grammar_model` 默认值为 `"none"`；按仓库 `AGENTS.md` 规定，跨平台对齐与单一宿主特性不得直接篡改共享核心的出厂默认值。
3. 本机安装目录（`~/Library/Application Support/Shurufa/resource-packs/`）尚未部署 `wanxiang-lts-zh-hans.gram`，且 373 MB 的上游模型文件不应塞进 `Shurufa.app` 安装包或备份目录。

## Decision

在保持跨平台共享默认值 `grammar_model == "none"` 不变的前提下，为 `Shurufa` 原生端接通万象 LTS（CC-BY-4.0）语法搭配模型，形成从安装脚本、一次性偏好迁移到原生设置开关的完整闭环：

- **原生设置开关（`platforms/macos/src/settings/AppearancePreferences.mm`）**：在 `Shurufa` 版本的「词库与数据 → 云端与智能候选」增加平铺开关「万象语法搭配」（defaults 键 `MSIMEClientSentenceAssociationWanxiangGrammar`），照 `neural_desktop` / `show_next_on_duplicate` 模式接入 `SharedOverrideProperties()`、`SettingProbes()`、读写合并、共享偏好回写和 `sentenceWanxiangGrammarChanged:`。开启时若尚未选择其他非 `"none"` 自定义模型则写入 `"wanxiang"`，并保留对用户已手动配置的其他非 `"none"` 模型（如 `"baguwen_compact"`）的兼容；关闭时切回 `"none"`。同时收窄 Shurufa 下独立自定义模型的禁用提示与迁移边界。
- **一次性默认启用迁移（`platforms/macos/src/settings/ShurufaPreferenceMigration.h` 与 `input_method_main.mm`）**：新增 `MSIMEMigrateShurufaGrammarModel`（`ShurufaGrammarModelMigrationV1` 标记），仅当 `MSIMEIsShurufaBuild()` 为真且共享偏好中的 `sentence_association.grammar_model` 仍为 `"none"` 时一次性升级为 `"wanxiang"`；用户之后手动关闭写回 `"none"` 或选为其他模型时，`ShurufaGrammarModelMigrationV1` 已落盘，后续重启与升级不再覆盖用户选择。同步接入 `Shurufa.app --migrate-grammar-model` 命令行入口。
- **单一数据目录存储与自动安装（`platforms/macos/scripts/install-shurufa.sh`）**：模型文件仅存放在 `~/Library/Application Support/Shurufa/resource-packs/grammar-model-wanxiang/wanxiang-lts-zh-hans.gram`，不打入 `Shurufa.app` 包内。`install-shurufa.sh` 安装新包前检测该路径是否缺失 `.gram`，缺失时调用 `msime-client-providers` 的 `install_resource_pack` 示例拉取安装（滚动 LTS 校验规则见[万象 LTS 原地替换不再阻断资源更新](../bug-fix/2026-10-26-wanxiang-auto-update.md)），装包后执行 `"$executable" --migrate-grammar-model` 完成首次迁移。

## Alternatives considered

- **把 `crates/client-core/src/preferences.rs` 中的 `grammar_model` 默认值直接改为 `"wanxiang"`**：只需改一处常量，所有平台自动开启；但它会静默改变跨平台共享偏好与上游未主动下载用户的默认行为，且缺少本地已装包与否的区分，违反仓库关于核心默认值的约束。
- **把 `wanxiang-lts-zh-hans.gram` 打入 `Shurufa.app/Contents/Resources/` 随包分发**：首次启动零网络依赖；但 373 MB 会让每次本地编译部署与安装包备份体积膨胀数倍，且无法复用 `resource-packs` 的滚动更新目录。

## Consequences

- `Shurufa` 用户在安装或升级后自动具备万象 LTS 语法搭配能力（配合[清洗非汉字前文消除神经模型 &lt;unk&gt; 污染并打通词格跨边界上下文](../bug-fix/2026-11-18-han-clause-context-sanitization.md) 中的 `collocation_tail_with_context` 跨句边界上下文与词格加分），且可通过「万象语法搭配」原生开关随时启停；每次词格重打分会在已加载模型后直接查 `GramDb` 内存索引，占用与运行特点见[语法模型读取端](2026-10-21-octagram-grammar-model.md)。
- 构建产物 `Shurufa.app` 不增加任何模型体积；首次运行 `install-shurufa.sh` 在未缓存模型时会联网拉取一次约 373 MB 文件到 `~/Library/Application Support/Shurufa/resource-packs/grammar-model-wanxiang/`。

## Verification

- `ShurufaPreferenceMigrationTest.mm` 覆盖 `"none" -> "wanxiang"` 迁移、关闭后再次执行保持 `"none"` 的幂等性，以及已有 `"baguwen_compact"` 配置不被覆盖，编译运行通过。
- `python3 platforms/macos/tests/settings/preference_coverage.py .`：`394 of 444 shared preferences reach the macOS host; the other 50 are accounted for (280 nested fields checked)`。
- `preference-snapshot-merge-test`、`preferences-lifecycle-test`、`local-mode-preferences-test`、`shurufa_brand_audit_test.py` 与 `./shortcut-test --shurufa-settings target/macos-isolated/Shurufa.app` 全部通过；`platforms/macos/build-shurufa.sh` 与 `verify_shurufa_bundle.py` 打包校验通过。
- `cargo clippy -p msime-client-providers --all-targets -- -D warnings`、`cargo test -p msime-client-providers --lib -- resource_packs`（36 项通过）与 `install_resource_pack ... grammar-model-wanxiang`（390,786,092 字节落盘至 `~/Library/Application Support/Shurufa/resource-packs/grammar-model-wanxiang/`）全部通过。

## Related

- [语法模型读取端](2026-10-21-octagram-grammar-model.md)：Rime `.gram` octagram 模型在词格与搭配尾词上的编码与窗口契约。
- [万象 LTS 原地替换不再阻断资源更新](../bug-fix/2026-10-26-wanxiang-auto-update.md)：万象资源包的 `Unintendedz/shurufa` 目录地址、内嵌兜底与滚动 LTS 校验策略。
- [Shurufa 随包提供停顿后精排模型](2026-11-08-shurufa-settled-rerank.md)：同款 macOS 原生平铺开关与 `AppearancePreferences.mm` 共享偏好探针模式。
- [清洗非汉字前文消除神经模型 &lt;unk&gt; 污染并打通词格跨边界上下文](../bug-fix/2026-11-18-han-clause-context-sanitization.md)：把前文最后汉字片段经 `collocation_tail_with_context` 喂给 `.gram` 语法窗口的前序改动。
