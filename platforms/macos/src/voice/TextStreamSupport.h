#pragma once
#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

// TextStream 本地流式识别：随包的启动器 Contents/Resources/textstream/shurufa-textstream 跑 Python 的 MLX 解码，协议与 msime-voice-local 相同。运行环境和模型由随包的 install-runtime.sh 装进 ~/Library/Application Support/Shurufa/textstream（设了 SHURUFA_TEXTSTREAM_ROOT 时用它，测试用）。
FOUNDATION_EXPORT NSString *MSIMETextStreamRootDirectory(void);
// 模型目录，直接含 config.json 和 model.safetensors。
FOUNDATION_EXPORT NSString *MSIMETextStreamModelDirectory(void);
// 安装脚本写下 .ready 标记、且模型文件都在时为真；不核对内容，内容由安装脚本按哈希校验。
FOUNDATION_EXPORT BOOL MSIMETextStreamReady(void);
// 启动器：MSIME_TEXTSTREAM_LAUNCHER 已设置时用它（测试指向源码树），否则是 bundle 里的那份。不是可执行文件时为 nil。
FOUNDATION_EXPORT NSString * _Nullable MSIMETextStreamLauncherPath(void);
// 随包的 install-runtime.sh，规则同 MSIMETextStreamLauncherPath（它是启动器的同目录文件）。
FOUNDATION_EXPORT NSString * _Nullable MSIMETextStreamInstallerPath(void);

// 运行随包的 install-runtime.sh：建 Python 环境、装依赖、放模型（离线时从 OpenLess 克隆，否则从 Hugging Face 下载），完成后写 .ready。在后台线程运行，`completion` 在主线程回调；`summary` 是脚本最后一行输出（成功）或失败原因，不含密钥。同一时刻只跑一个，已有一个在跑时直接以失败回调。
FOUNDATION_EXPORT void MSIMETextStreamInstall(void (^completion)(BOOL success, NSString *summary));
// 安装是否正在进行。
FOUNDATION_EXPORT BOOL MSIMETextStreamInstalling(void);

NS_ASSUME_NONNULL_END
