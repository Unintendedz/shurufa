# Agent Note: 语音输入的加密存档与听写历史

Status: implemented

## Problem

语音输入一旦被打断（识别服务失败、超时、进程崩溃、用户误按 Esc），说过的话就没了，只能重说一遍。识别成功的文字也只上屏一次，事后找不到。维护者要求：每次录音都留下来，失败、中断、成功的都留，方便事后重新听写；文字历史默认永久保留、可以设期限；录音轮换保留（默认 24 小时）；全部加密，不能被轻易读到；换一台 Mac 装上就能用，不需要手动配置。

## Decision

每次开麦都写一份加密存档，存储格式、加密和保留期限放在共享 Rust 层，macOS 宿主只管密钥、采集和界面。

### 存储（`crates/client-providers/src/voice/archive.rs`）

- 存档目录是状态目录下的 `voice-archive/`（目录 0700，文件 0600）。每次录音两份文件：`<id>.audio` 是一串独立密封的块，每块约半秒 16 kHz 单声道 16 位 PCM；`<id>.record` 是听写记录（JSON）整份密封。
- 加密用 XChaCha20-Poly1305（与 client-core 云剪贴板同一版本），每块随机 24 字节 nonce。aad 绑定 id 和块序号，块不能换序、不能挪到别的录音；记录绑定 id。
- 录音一边进行一边落盘：开始时先写好记录和音频文件头，进程崩溃或被杀时，最后一个完整的块之前都在，读到不完整的尾块就停。这样的记录状态是「中断」。
- 记录里的识别结果按到达顺序追加（`segments`），另有 `failure`、`cancelled`、`retranscription`。列表里显示的状态由记录推出：有文字为「已识别」，否则失败、取消、中断、无结果。
- id 是 `<13 位毫秒时间戳>-<32 位十六进制>`。保留期限按 id 里的时间判断，清理不用解密；操作文件前先校验 id，不合规的一律拒绝。

### 保留期限（共享偏好）

`voice_input.recording_retention_hours` 取 24（出厂）或 72，过期只删音频；`voice_input.history_retention_days` 0 为不限期（出厂），否则 1..=36500，记录过期时连同音频一起删。宿主在存档打开时、每次录音结束后和修改期限后各清理一次，正在录的那一条不删。

### C ABI（`crates/host-api/src/ffi/voice_archive.rs`）

`msime_client_voice_archive_{open,begin,append,end_capture,note,list,audio,delete,purge}`。进程里只有一个存档和至多一条正在录的录音，状态放在全局锁里，追加可以在采集队列上调用。解密后的音频经回调交给宿主一次，不落盘。

### macOS 宿主

- 密钥：登录钥匙串的通用密码条目 `dev.shurufa.voice-archive`，`kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`，不随 iCloud 同步。第一次使用时没有就用 `SecRandomCopyBytes` 生成；读失败但不是「没有」（钥匙串锁着、被拒）时 must never 生成新密钥，存档保持关闭，下次再试，免得旧记录再也解不开。
- 采集：`MSIMEVoiceInputService startMicrophoneCapture` 是所有识别方式（TextStream、本地模型、豆包、HTTP 整句、系统语音）共用的那一个麦克风 tap，在这里另接一个 16 kHz 转换器，把每个缓冲送进 `MSIMEVoiceArchive` 的串行队列；停止采集时收尾。不到 1/4 秒的误触（`short_capture`，本来就不识别）连同记录一起删掉。外部 provider socket 自己采集，不经过这里，不存档。
- 结果：`applyVoiceResult:` 是所有识别方式上屏的唯一入口，在这里追加文字；`reportVoiceFailure:` 追加失败原因，只记开麦之后才会发生的那几类（录音、识别、无语音、超时、会话）；三处用户取消（自定义快捷键的 Esc、按住说话的 Esc、浮层的取消按钮）记取消。都按服务的 `archiveRecordingID` 记，它在每次 `startWithSession:` 时清空、开麦时设上，开麦前的失败不会记到上一条上。
- 存档经 `MSIMEVoiceArchiving` 协议交给 `MSIMEVoiceInputService.archive`，由输入法进程启动时设置。单元测试不设置，所以测试程序不碰钥匙串和磁盘；`VoiceArchive.mm`、`VoiceHistoryView.mm` 只编进输入法本体。
- 设置窗口新增「听写历史」页（`MSIMEVoiceHistoryView`，按类名在运行时取）：保留期限、记录列表（时间、时长、应用、状态），播放、复制、重新听写、删除。重新听写用 `MSIMEVoiceRecognitionQuery` 按当前设置建同一种识别请求，把存档里的 PCM 按 0.1 秒一块送完，开了整理就照常整理，结果记进 `retranscription`。录音时的控制器也改用这个函数建查询，两边不会走偏。

## Alternatives considered

- 只在失败或中断时保存录音：占用更少。但维护者明确要求成功的也留，而且「失败」在很多路径上是事后才知道的，一边录一边写才扛得住崩溃，所以全部存、按期限轮换。
- 用 SQLite（SQLCipher）存记录：查询方便。但要多引入一个带 C 依赖的加密 SQLite，而且音频块本来就是文件；每条一份密封文件已经够列表和清理用，按 id 里的时间清理不用解密。
- 在 Objective-C 里用 CryptoKit/CommonCrypto 加密：少一层 FFI。但 CommonCrypto 没有公开的 AEAD，CryptoKit 只有 Swift 接口；仓库已有同一套 XChaCha20-Poly1305 和测试方式，格式放在 Rust 里也能直接用 cargo 测。
- 密钥放进 iCloud 钥匙串，换 Mac 后历史还能读：需要带 Team ID 的签名和钥匙串访问组，本机自签名证书做不到。维护者接受换 Mac 后历史从空开始。

## Consequences

收益：任何一次录音都能事后回放和重新听写，包括识别失败、超时和进程崩溃的那些；文字历史可查可复制；磁盘上只有密文，密钥只在本机钥匙串里；新 Mac 上构建安装后第一次录音就自动生成密钥，不需要配置。

代价：每分钟录音约 1.9 MB，默认 24 小时后删除；文字默认永久保留，长期会积累很多小文件。钥匙串里的密钥丢了（删掉条目、换机器）旧记录就解不开，列表只报「无法解密」的条数。外部 provider socket 和过短的录音不存档。重新听写不支持系统语音识别和外部 provider socket。

测试：`voice::archive` 12 个单测覆盖往返、文件里没有明文、中断后保留完整块、换密钥和篡改被拒、记录不能换 id、状态推导、两种期限和正在录的那一条、id 校验、删除、文件权限；host-api 的 `a_recording_flows_through_the_c_abi` 走一遍完整的 C ABI；client-core 覆盖两个期限的出厂值和校验。macOS 侧的钥匙串、采集和设置页需要装机后实测。
