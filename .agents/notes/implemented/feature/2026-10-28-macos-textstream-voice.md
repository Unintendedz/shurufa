# Agent Note: macOS 本地流式识别 TextStream 作为默认语音服务

Status: implemented

## Problem

macOS 宿主的本地识别只有 `msime-voice-local`（sherpa-onnx）一条路，需要用户先下载模型目录，而且识别和整理的效果不如用户平时在 OpenLess 里用的 TextStream（Qwen3-ASR + MLX，边说边出字）加大模型整理。要让 Shurufa 在本机直接获得同样的体验，并且不让输入法进程自己持有模型。

## Decision

- 新增 ASR 服务 `textstream`，并作为未设置时的默认服务。识别在独立进程里：`platforms/macos/textstream/shurufa-textstream` 启动 `textstream_worker.py`，协议逐条兼容 `msime-voice-local`，所以宿主复用 `MSIMELocalVoiceRequest`/`MSIMELocalVoiceHelper`，只换可执行文件和模型目录。`MSIMELocalVoiceHelper` 变成按通道各一个单例（`shared` 与 `textStream`），两种本地服务切换时各用各的进程，互不顶替。
- 运行环境不进包：`install-runtime.sh` 随包，由设置窗口的“安装语音模型”调用，用 uv 建 Python 3.12 虚拟环境并按带哈希的 `requirements.lock` 安装，模型放在 `~/Library/Application Support/Shurufa/textstream/model`（OpenLess 已装时 APFS 克隆，否则从 Hugging Face 固定 revision 下载，逐文件校验 sha256），完成后写 `.ready`。就绪判定只看 `.ready` 和模型文件，不用 `msime-model.json`。
- 解码间隔默认 0.5 秒（textstream-asr 默认 2.5 秒）：首个 partial 约 0.2–0.3 秒，CPU 约占一核的 15%；准确率在合成语音上各间隔无差别。
- 整理沿用现有 `polish_*` 链路，`textstream` 下默认开启（用户没存过开关时按开，仍要求有凭据才发请求）；整理失败或超时原样上屏识别文字。新增只能调小的 `polish_timeout_ms` 选项，仅为测试超时路径。
- 从 OpenLess 导入是输入法设置窗口里由用户触发的动作：读登录钥匙串里 `com.openless.app`/`credentials.v2` 的凭据（系统会弹授权框），读 `preferences.json`、`style-packs.json`，把当前整理风格的提示词写进自定义方案 1 并选中。不读历史、录音、速记。提示词取值顺序固定：内置风格的用户覆盖值，其次 style-packs.json 里同 id 的包，最后 `styleSystemPrompts`。

## Alternatives considered

- 把 MLX 识别直接放进输入法进程：最强的理由是省一个进程和一层协议。但输入法被加载进每个接受文字的应用，模型和 Metal 内存会拖慢前台应用，崩溃会带走打字；AGENTS 和 `msime-voice-local` 的设计都是为了避免这一点。
- 新写一套宿主请求类给 TextStream：最强的理由是不碰 `LocalVoiceRequest.mm`。但协议相同，重写只会复制进程管理、超大行和取消处理，两份实现日后要分别修。
- 把 Python 运行环境打进 app 包：最强的理由是首次使用免安装。但依赖含 Metal 运行库约 100 MB，模型 1.9 GB 无论如何要另装，签名和公证也要处理 Python 的二进制；先走随包脚本安装。
- 用 JSON 外的启发式去猜 OpenLess 凭据载荷：不做。凭据载荷按 JSON 对象树里的键名读取（`ark.api_key` 等），读不到就报错，不猜。

## Consequences

默认服务从 `local` 变成 `textstream`：新用户未安装模型时，第一次按语音热键会提示去设置里安装；已显式选择 `local` 的用户不受影响。`textstream` 需要 `uv`（Homebrew 或 ~/.local/bin）和网络（或本机已有 OpenLess 模型）。凭据载荷的真实结构在开发机上没能核对（读取需要用户在钥匙串授权框点“允许”），解析按键名在对象树里查找，结构与预期不符时导入会报“没有完整的 Ark 配置”，需要按真实结构调整 `arkFieldsFromPayload`。
