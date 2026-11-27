# Agent Note: 性能与质量评测先验证真实资源身份

Status: implemented

## Problem

固定`bd4452e150`的`rerank_latency`与`convert_eval`读取当前资源锁的`source_commit`，却不校验`--resources`的内容，直接用该提交号准备代次。被称为verified的历史评测目录实际上缺少bigram/trigram，拼音库73711616字节、英文库1667072字节，不匹配当前锁的73723904、4526080字节及SHA-256。两项缺失会改变产品解码成本和质量，不能将该目录的测量当作当前完整产品验收。旧目录保留，不覆盖历史证据。

## Decision

两个评测入口共用已有`ResourceSet`与`ResourceStore::verify`，准备用户代次、读取用例及预热前验证目录中全部锁定文件的类型、长度与SHA-256。默认嵌入编译时的仓库资源锁，移动程序或源码随后更新都不会悄悄换掉清单；显式`--resource-lock`允许使用可复现的自定义资源集合，不提供绕过校验的开关。沿用现有`helpcodes/`真实目录放行例外，其内容不属于清单指纹；测量必须另记辅助码资源来源或缺席。校验不安装、不复制或修改资源，不进入逐键计时。报告到stderr的source_commit与资源清单generation用于区分输入身份，质量JSON格式和模型可选行为在明确自定义锁时保留。

## Historical audit

检索活跃笔记的评测、资源、verified-dir、MSIME_EVAL_RESOURCES与资源哈希：现有性能笔记部分重叠，持有局部算法及计时结论，没有入口资源认证契约。[在线缓存回写](../../implemented/testing/2026-10-19-online-cache-in-place.md)的固定旧函数、分配和容量证据仍成立，其旧目录全量延迟不能替代完整产品基线。[全词表投影](../../implemented/testing/2026-10-16-runtime-project-row-stream.md)保留上游数值及完整局部预算；本片不翻转算法决定，不批量改写旧记录。`client-core::resources`已经实现严格目录和SHA校验，复用现有代码，不新建散列或安装器。没有完全吸收的笔记或过时提案。

## Alternatives considered

- 只由调用者承诺verified：支持任意目录且启动成本低，但已经产生缺少n元表及旧库冒充当前锁的实际测量，入口需验证承诺。
- 自动修复或下载资源：可减少人工准备，但会改变被测输入、引入网络并掩盖旧证据，入口只验证并明确失败。
- 永久拒绝自定义资源：默认约束简单，但现有模型缺席及替换实验需要显式可复现输入，因此允许额外资源锁，仍逐项校验。
- 仅检查文件名和大小：成本低，但等长损坏与替换无法发现，复用已有SHA验证。

## Verification protocol

旧正式二进制在缺失/过期目录上必须先真实未能拒绝。合成锁测试覆盖匹配、不存在、缺文件、等长损坏、长度不符、未声明文件与无效清单，并确认校验不写资源。两个CLI的默认锁拒绝旧目录，自定义锁通过校验后仍走原模型及会话语义。完整当前锁资源的旧/新质量JSON完全相同；重新保存81例1791键及16ms预算的正式延迟基线。跑相关测试、Clippy、fmt、notes、quick及正常pre-push，审查后单独PR合并。

## Consequences

完整资源哈希增加一次启动读取，必须在计时与会话准备之外。自定义模型目录须有相应锁文件；清单缺文件或包含额外文件会明确失败，不能静默降级。此片修复测量输入身份，不宣称产品提速或九键16ms达标。

## Initial evidence

最初在工作树改名后直接调用此前编译的程序，因旧程序内嵌的`CARGO_MANIFEST_DIR`指向已移动目录，得到锁文件不存在错误；这两次不属于有效红测试或质量基线。重新在最终路径构建未改生产代码后，两入口用旧目录、单例、零预热实际均退出0且生成统计，资源拒绝断言失败。默认改为编译时嵌入锁也消除这项路径依赖。接入后两个例程的共享资源测试全部通过，分别10项与7项；其中转换例程另有3项既有网格测试。

## Verification

在最终工作树路径重新构建未改的旧程序，两入口使用旧资源目录、单例、零预热均退出0并生成统计，拒绝断言失败。接入校验后两个正式release入口均非零退出，列出缺少的8项锁定文件，标准输出为空。新入口完整校验当前锁的12项资源；全拼60例、九键81例的新旧质量JSON整体相等。显式自定义清单去掉模型，资源也仅去掉模型，3例转换结果与旧程序JSON整体相等，模型可选语义保留。

主机两个例程测试分别10/7项通过；`cargo clippy --locked -p msime-input-runtime --all-targets -- -D warnings`、release构建通过。独立只读审查无阻断，要求收窄`helpcodes/`的覆盖表述，已按实际校验范围修正文档。

Android API 28专用AVD `emulator-5566`上，`build-core-test.sh`整套完成：client-base 36通过、2忽略、3过滤；client-core 399通过、1忽略；client-providers 222通过、2忽略；client-services主程序1067通过、9忽略、4过滤，另有两个子进程专项各1通过。硬链接用例按既有runner跳过，没有新增豁免。

Android新增例程测试在同一API 28 AVD分别10/7项通过。原生full瘦包以`MSIME_ANDROID_OMIT_ON_DEMAND=1`构建，省略按需资源，不把Tauri合包当产品。首次Gradle启动受默认Java 25.0.2影响失败，显式切换已安装的JDK 21后构建、16KB对齐与开发签名通过。`smoke.sh emulator-5566 --core`实际通过提交删除、批量组字及中英切换、数字层重启状态和分词分享等步骤，但在已有`CopyDeviceInfoDeviceSmoke`的剪贴板等待阶段失败；单项复现仍失败，失败前截图显示菜单仍在第一页并切换了单手模式，没有到达设备信息复制入口。此片没有修改Android宿主或设备用例，不能把本次整套装包验收写成通过；菜单翻页问题另片追踪，不放宽断言或新增已知失败豁免。

本地`TMPDIR=$PWD/target/tmp CARGO_BUILD_JOBS=4 bash scripts/verify-local.sh --quick`退出0：157项Windows本机测试、契约检查、HarmonyOS设置bundle、Rust workspace、Android交叉编译及Java宿主、Wasm编译与Clippy、Linux共享桌面组件、Linux原生宿主90项测试、macOS原生工程和Apple bridge编译通过。MinGW未安装，Windows完整交叉编译及pipe-only跳过；HarmonyOS缺hvigorw与暂存产物，真打包跳过。macOS桌面bundle在workspace检查时尚未构建，未参与该阶段；没有进行macOS系统输入法或真机验收。

## Verified resource baseline

[完整结构化记录](../../../../docs/measurements/2026-10-19-nine-key-verified-resources.json)保存资源清单、来源提交、资源代次、二进制及用例SHA-256、质量整体相等证据、逐秒进程数量和负载。Mac13,1、10逻辑核、Rust 1.97.1上使用完整12项锁定资源，`helpcodes/`缺席。81例、每臂1791键、5例预热、交替先后，预算仍为16ms，分位数采用nearest-rank。engine-only mean/p95为4.80/9.53ms，with-model为7.87/20.13ms，配对重排增量mean/p95为3.08/12.45ms；226键（12.6%）超过16ms，p95门禁按原规则非零退出，未重测找绿。

本片构建全部结束后采样；24次监测中cargo、rustc、clang最大数量分别为0/0/0，一分钟负载12.91–15.48。这不证明机器独占。该结果是纠正被测输入后的完整资源基线，不与旧残缺资源解释为产品算法回退或提速；九键16ms目标仍未达成。

后续追踪见[分页网格手势修复](../bug-fix/2026-10-19-android-paged-grid-intercept-delta.md)：确定性设备回归确认首段位移丢失，但没有证明它是本文复制超时的唯一原因；本文保留当次装包失败事实。端侧两阶段真实神经模型评测门禁 `tier1_eval` 对本资源校验契约的复用见[端侧两阶段真实模型自动化评测门禁](2026-11-27-tier1-neural-end-to-end-eval-gate.md)。
