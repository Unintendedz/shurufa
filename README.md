# Shurufa

> 一个输入法，它首先要能打字。

Shurufa 是我自己用的 macOS 中文输入法，结合了[水杉输入法](https://github.com/metasequoiaime/msime)和 [OpenLess](https://github.com/Open-Less/openless)：打字的部分来自水杉，语音输入的体验和本地流式识别的做法来自 OpenLess。

**这只是一个个人 fork 的项目，它只适合我自己的设备（目前只在 Mac 上用）。**

## 为什么要 fork

水杉是一个很有野心的项目：六个平台、几十种快捷模式、云同步、插件、皮肤，要照顾各种各样的用户。而我对输入法的要求朴素得多：先在我这台 Mac 上把字打好，再加上顺手的语音输入。所以我把它 fork 下来，关掉用不到的，加上我想要的。

## 我的一点看法

在 AI 时代，一个仓库要去适配所有人机器上的各种边缘情况，是非常费劲的：每多一个平台、一个选项、一种用法，就多一份要验证、要维护的组合，最后谁都照顾不周全。反过来，改代码的成本已经低了很多，在别人的软件基础上改出一个只给自己用的版本，往往比等上游照顾到你的情况快得多。

所以我鼓励大家：遇到好用的开源软件，fork 下来，针对自己的机器和习惯去改造它。

## 这个仓库的约定

- **不接 PR。** 一切以我自己用得爽为准。
- **没有支持多平台的打算。** 目前我只在 Mac 上用它。仓库里其他平台的代码是从水杉继承来的，不维护，也不保证还能编译。
- **不跟随上游。** 分叉点是 tag `fork-point/msime-2026-10-27`，之后的上游提交不合并。

## 它现在是什么样

- 只做中文：全拼、双拼组字，神经网络排序，候选窗。英文交给系统自带的 ABC，Caps Lock 由系统切换。
- 语音输入：按快捷键说话，本地流式识别（TextStream：Qwen3-ASR + MLX），可以接大模型整理文字；每次录音加密存档在本机。
- 几个顺手的小功能：打完拼音按 Tab，按笔画或部件筛候选；全拼里打 `v` 再接算式、数字或日期，直接出结果、大写金额或日期格式。
- 关掉了使用统计和云联想。

## 自己构建

只在我的环境里验证过：Apple Silicon、Command Line Tools 的 MacOSX26.5 SDK、Rust、Homebrew（CMake、Ninja、python@3.14）。

```sh
bash platforms/macos/build-shurufa.sh          # 构建 target/macos-isolated/Shurufa.app
bash platforms/macos/scripts/install-shurufa.sh  # 安装到 ~/Library/Input Methods/Shurufa.app
```

构建按登录钥匙串里的本机自签名证书「Shurufa Local Signing」签名，没有时脚本会自己建一张。换到你的机器上大概率要改点什么，这正是上面说的：fork 下来，按你自己的来。

## 致谢

- [水杉输入法（MSIME）](https://github.com/metasequoiaime/msime)：输入引擎、各平台宿主框架和词库都来自它。谢谢水杉的作者和贡献者，没有这么扎实的底子，就没有这个 fork。
- [OpenLess](https://github.com/Open-Less/openless)：语音输入照着它的体验来做，本地流式识别 worker 的适配器结构、Metal 内存与 CPU 预算、VAD 门控改编自 OpenLess 的 TextStream 适配器。谢谢 OpenLess 的作者和贡献者。
- 其他第三方组件见 [docs/third-party.md](docs/third-party.md) 和 [THIRD_PARTY_NOTICES.txt](platforms/macos/resources/Licenses/THIRD_PARTY_NOTICES.txt)。

## 许可证

[GPL-3.0-only](LICENSE)。改编自 OpenLess（AGPL-3.0）的部分，来源与许可见 [THIRD_PARTY_NOTICES.txt](platforms/macos/resources/Licenses/THIRD_PARTY_NOTICES.txt)。
