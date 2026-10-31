# Agent Note: Shurufa 本地构建写入源码修订

Status: implemented

## Problem

Shurufa 版本线独立维护后，本机安装的 bundle 一律报 0.55.0、构建号 1，无法追溯到具体提交；2026-10-31 的一次安装还是由两个分支的产物拼出来的，装机之后没有任何办法从 bundle 本身看出它含哪些改动。

## Decision

`platforms/macos/build-shurufa.sh` 在配置 CMake 时把 `git rev-parse HEAD` 通过 `MSIME_INPUT_SOURCE_REVISION` 传入，由 CMake 写成 Info.plist 的 `MSIMEInputSourceRevision`；该键的定义和校验沿用[发布包的资源刷新](../bug-fix/2026-10-22-macos-prerelease-resource-refresh.md)。工作区有未提交改动时脚本向标准错误报告并传空值，bundle 不写修订，显式传空也会清掉缓存里上一次的值。`install-shurufa.sh` 装完后打印该键，缺省时打印「未记录」。

没有修订的 bundle 表示构建时工作区不干净，这是刻意的信号：一个脏构建不能声称自己是某个提交。

## Alternatives considered

- 每次构建递增 `build-number.txt`：最直接，每个包有不同编号。否决：版本号和构建号是发布决策，还会驱动 Sparkle 与设置应用的比较，本地构建不应改它。
- 脏工作区写成 `<sha>-dirty`：信息更多。否决：该键按十六进制校验，设置应用也用它比较，非十六进制值会让配置失败或误导比较；不写才是诚实的信号。

## Consequences

收益：安装后可由 `MSIMEInputSourceRevision` 对回提交。代价：脏构建仍无法追溯，需要先提交再构建；该键只记录提交，不记录构建时的未提交内容。
