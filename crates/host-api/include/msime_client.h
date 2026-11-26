#ifndef MSIME_CLIENT_H
#define MSIME_CLIENT_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

/* Host-neutral focus lease and key event contract. Values are opaque to the ABI. */
typedef struct msime_client_focus_lease {
  uint64_t client;
  uint64_t epoch;
  uint64_t token;
} msime_client_focus_lease;

typedef struct msime_client_key_event {
  msime_client_focus_lease lease;
  uint32_t virtual_key;
  uint32_t scan_code;
  uint32_t modifiers;
  uint32_t character;
  bool ui_less;
} msime_client_key_event;

/* Outcome for platform key-router writes. Only definitely-not-sent may use a
 * local fallback; ambiguous delivery must wait for lease recovery. */
typedef enum msime_client_key_dispatch_result {
  MSIME_CLIENT_KEY_SENT = 0,
  MSIME_CLIENT_KEY_DEFINITELY_NOT_SENT = 1,
  MSIME_CLIENT_KEY_DELIVERY_AMBIGUOUS = 2,
} msime_client_key_dispatch_result;

static inline bool msime_client_key_dispatch_allows_fallback(
    msime_client_key_dispatch_result result) {
  return result == MSIME_CLIENT_KEY_DEFINITELY_NOT_SENT;
}

static inline bool msime_client_key_event_valid(const msime_client_key_event *event) {
  return event != NULL && event->lease.client != 0 && event->lease.epoch != 0 &&
         event->lease.token != 0 && event->virtual_key <= 0xff &&
         (event->modifiers & ~UINT32_C(0x0f)) == 0;
}

/* ABI 3 (3 replaced msime_client_builtin_skins with the global theme functions). All functions return owned, NUL-terminated UTF-8 JSON. Free exactly once using msime_client_string_free, including error responses. Never use free(). Responses: {"ok":true,"value":...} or {"ok":false,"error":"..."}. Creation returns a View; its session field is the handle. Handles are confined to their creating thread. Dispatch, focus, view and destroy on that thread. Text and candidate values are copied; no Engine pointers escape. */
uint32_t msime_client_abi_version(void);
/* Worker-thread bootstrap: {resources: absolute path, state_root: absolute path}.
 * Verifies pinned resources, delegates working data preparation to Engine and
 * returns HostOptions. Maximum 16384 bytes; no session may use state_root during
 * preparation. Caller publishes the returned config atomically after success.
 * 可选的 edition 是版本 id（shared/contracts/editions.json），缺省为 full；不是 full 时返回的 HostOptions 带 edition 键，未知 id 返回错误。
 * 可选的 runtime_options 是已发布的 runtime-options.json 的绝对路径：准备成功后，文件里的 dictionaries 不是这次准备的代次时改写成它，其余键不动；改写失败只记诊断，不影响返回值。Windows Server 每次启动都重新准备、会话用返回的文档，靠它让照这份文件工作的设置窗口和 TIP 跟上同一个代次。
 */
char *msime_client_prepare_host(const uint8_t *options, size_t length);
/* path is an absolute UTF-8 runtime options file path of length bytes; maximum 4096. When its dictionaries directory is not the installed resource generation (after a package upgrade), prepares that generation, replays the user dictionary into it and atomically rewrites resources/dictionaries, keeping every other key. It also keeps language_dictionaries in step with the Cantonese, Zhuyin and Stroke dictionaries installed beside the resources, whatever the generation; only an input method host may call it, because a host older than this library rejects that key. Value is true when the file was rewritten. Call before creating any session from the file. When the recorded resource directory does not match the compiled dictionary lock (downloaded dictionaries an upgrade did not replace) the error text begins with "dictionary_outdated:" and the file is left unchanged; the rest of that text may name private paths. */
char *msime_client_refresh_host(const uint8_t *path, size_t length);
/* options is a readable UTF-8 buffer of length bytes; maximum 1 MiB.
 * Object: api_version=1, resources/user_data/cache/dictionaries (absolute paths),
 * preferences={scheme, candidate_page_size, learning, chinese_punctuation,
 *              shuangpin_profile?, shuangpin_custom_profile?, wubi_profile?}。缺省 shuangpin_profile 为 xiaohe，
 * 可选 xiaohe、ziranma、shoudao、microsoft、smart_abc、custom。custom 读 shuangpin_custom_profile（{initials, finals, zero_initials}，各是单位到键的对象），表缺失或不合法时按 xiaohe 建会话。缺省 wubi_profile 为 wubi86，可选 wubi86、wubi98。不认识的取值一律拒绝。
 * Optional preferences_directory is bootstrap metadata for host file monitoring;
 * session creation itself does not monitor or load it.
 * Optional phrase_preedit=true asks for a half-composed phrase to stay in the
 * composition: picking a candidate that covers only part of the input leaves the
 * chosen piece in view.phrase_prefix instead of committing it, and the whole
 * phrase commits at once when the composition ends. The host must draw that
 * field ahead of editing_text - it is separate because caret_position is an
 * offset into editing_text in the host's own string unit. Omitted means each
 * piece is committed as it is picked, as before.
 * The host must prepare and validate its dictionary generation before creation.
 * Creation acquires cooperative shared access to user_data and dictionaries until
 * destroy. It fails immediately while a participating maintenance writer holds
 * exclusive access. Existing sessions are never cancelled for maintenance.
 * Linux hosts may provide absolute online_provider_socket and
 * translation_provider_socket paths for user-managed Unix-socket services;
 * translation may reuse the online socket when omitted.
 * preferences_directory also names the plugins root, <preferences_directory>/plugins: the
 * installed plugin packs, the enabled command tables the "/" mode reads and the "@" mode's
 * mentions.json. They are read at creation, on preference updates and when a field gains
 * focus, and only while the mode that uses them is on. Optional sound_packs is the absolute
 * path of the bundle's built-in sound packs; absent means sound-packs beside resources.
 * Optional plugin_packs is the absolute path of the official plugin packs the desktop
 * installers ship (<kind>/<id>/ folders: the uuid script plugin, the latex-symbols phrase
 * table); absent means plugin-packs beside the built-in sound packs, and nothing when that
 * directory is not there (mobile, development builds without them).
 * Do not delete .msime-dictionary-access.lock files. Legacy/external writers do
 * not participate; preparation/upgrades still require stopped sessions.
 */
char *msime_client_create(const uint8_t *options, size_t length);
/* Management JSON (<=2248576 bytes), trusted native caller only:
 * {options: <same HostOptions as create>, action: {operation:"list",offset:0,limit:100}}
 * List 的 kind 与 query 可选：ASCII 查询匹配编码前缀，含非 ASCII 字符时匹配词条字面内容。无查询时列个人词条；指定 kind 与非空查询时包含内置行，quick_phrase 空查询也包含内置行。个人行优先；user_only:true 始终只列个人词条，在整库筛选后分页。
 * or action:{operation:"edit",previous:null|Entry,replacement:null|Entry,request_id:"..."}.
 * Batch import: action:{operation:"import",kind:"pinyin"|"wubi"|"wubi98"|"quick_phrase"|"english",
 * format:"standard"|"windows"|"rime"|"hans",text:"word<TAB>code<TAB>weight\\n",request_id:"..."}.
 * Standard rows are word, code, weight; Windows rows are code, word, weight. Rime rows are
 * word, code, optional weight and may include YAML front matter between --- and ...; Rime's
 * metadata weights (for example c=3 d=0.12) use the default 10000. Omitted weights use 10000.
 * The hans format is pinyin-only and accepts one pure Han phrase per line; Engine resolves
 * each phrase to its highest-ranked canonical pinyin and uses weight 10000.
 * Import accepts at most 1000 rows and returns {applied}; rows are committed
 * with deterministic receipt IDs derived from request_id so retries are safe.
 * Export: action:{operation:"export",kind,format:"standard"|"windows",offset,limit} returns
 * {text,has_more}; use pages of at most 1000 rows. Standard output is word, code, weight.
 * Entry:{kind:"pinyin"|"wubi"|"wubi98"|"quick_phrase"|"english",key,value,weight,source?:"user"|"bundled"}.
 * List returns {entries,has_more} and sets source on every entry; edit returns {applied:true}. Errors are redacted.
 * A bundled entry passed back as previous can only be re-weighted (replacement with the same kind, key and value) or deleted (replacement null); anything else fails with "bundled dictionary entry is read-only". Export of pinyin also carries the weights set or learned for bundled words and omits single characters; the other kinds export user words only.
 * 计数（只读）：action:{operation:"count",kind?,user_only?:bool} 返回 {count,kinds:{kind:n},complete}；user_only:true 只数用户自己的词，否则拼音还会数上学到或设过权重的随包词（也就是导出的那些行）。扫描在 1,000,000 行处停下时 complete 为 false。
 * 快照导出：action:{operation:"export_snapshot",destination:绝对路径,include_learning?:bool} 把用户的词写成与 GET /v1/users/me/dictionary/snapshot 相同的 NDJSON msime-dictionary-snapshot v1 文档（header、每个词一条 entry 和一条 overlay、带正文 SHA-256 的 footer；revision 为 1），用云端格式自己的校验器检查后返回那份元数据加上 path。不需要账号。include_learning 缺省为 false，这时不含位置和选择，输出与这个参数加入之前逐字节相同，云同步上传用的就是它；为 true 时（本地备份用）在用户的词之后再写输入记录：学习调权写成 user_inserted:false 的 overlay、删除记录写成 deleted:true 的 overlay、固定位置写成 position、选词计数写成 selection（截到 0..10），格式装不下的行跳过，返回值多出 learning 和 learning_skipped 两个计数；日志整体读不出来时照样导出词，learning 为 0，原因在 learning_error。
 * 输入记录：action:{operation:"learning_count"} 只读，返回 {count}，即本机日志里学习调权、删除记录、固定位置和选词计数的条数（不含用户自己的词）。action:{operation:"queue_learning_merge",source:绝对路径} 校验快照文件 source，把其中的输入记录另存成 <preferences_directory>/pending-learning-merge.ndjson（替换已有的一份），返回 {queued,learning}；没有输入记录时 queued 为 false、什么也不写。action:{operation:"merge_pending_learning"} 在没有会话时合并这份文件（要独占维护权，忙时报 dictionary maintenance busy、文件留着）：本机已有的词、固定的词和被占用的位置保留本机，选词计数取较大的那个，返回 {merged:false}（没有待合并的）或 {merged:true,entries,positions,selections,kept,skipped}；合并期间新排的一份不受影响，连续失败三次放弃并删掉文件（learning merge abandoned）。
 * 本地备份的校验与输入习惯：action:{operation:"inspect_snapshot",source:绝对路径} 只读地按云端格式完整校验一份词库快照并返回它的元数据。action:{operation:"export_habits",destination:绝对路径} 把日志里的输入习惯（整句联想的二元/三元计数、连续选词对、拼写纠错计数、自动纠错抑制、置顶的候选）写成 NDJSON（header format:"msime-learning-habits" version:1、每条一行、footer 带条数和正文 SHA-256），装不下的行跳过，返回 {path,habits,skipped}。action:{operation:"inspect_habits",source} 只读校验，返回 {habits}。action:{operation:"queue_habits_merge",source} 校验后存成 <preferences_directory>/pending-habits-merge.ndjson，返回 {queued,habits}；merge_pending_learning 随后在空闲时一并合并它（计数取大、置顶保留本机，写完压回各表上限），结果多一个 habits 字段。learning_count 另返回 habits（输入习惯行数，读不出来时为 null）。
 * 五笔查码（只读）：action:{operation:"wubi_codes",text} 返回 {phrase,characters,truncated}。characters 按 text 里第一次出现的顺序列出每个不同的汉字（最多 64 个，超出时 truncated 为 true），每项是 {text,wubi86:[...],wubi98:[...]}，编码短码在前、码表里没有时为空数组；phrase 是整段文字作为一个词的同样一项，只在整段都是汉字、2 到 16 个字、且至少一张表有它时给出，否则为 null。读会话用的同一份五笔码表，不拿维护权；text 超过 4096 字节时报 invalid wubi lookup。msime_client_personal_dictionary_request 对它给出同样的回答，不进队列。
 * Native host owns/authorizes paths; never accept arbitrary webview paths or log payloads.
 * Run on a worker thread. Edit returns busy until all participating sessions are
 * destroyed, then holds exclusive access; recreate sessions after success.
 * Do not automatically cancel user input to obtain access. Retry ambiguous writes
 * with the identical nonempty request ID and content.
 */
char *msime_client_dictionary(const uint8_t *request, size_t length);
/* The same {options, action} as msime_client_dictionary, as a settings page sends it: an edit,
 * import or reset first has the input hosts let go of the dictionaries (the Windows Server
 * over its auxiliary pipe) and is retried until they have; an import of a whole file (up to
 * 48 MiB of text) is sent in batches and answered with one report; and a refusal's error is
 * the settings pages' code: dictionary_busy, dictionary_too_large, dictionary_import_rejected,
 * dictionary_read_rejected, dictionary_pinyin_unavailable, dictionary_unavailable,
 * dictionary_reset_rejected, dictionary_bundled_readonly, dictionary_invalid_entry,
 * dictionary_invalid_word or storage. {operation:"reset"} restores the bundled dictionaries
 * and clears all learned data, returning {reset:true}. Blocks: call off the UI thread. */
char *msime_client_settings_dictionary(const uint8_t *request, size_t length);
/* 与 msime_client_settings_dictionary 相同；输入宿主的维护租约第一次立起来时，在调用线程上调用一次 announce(context)，
 * 让同一进程里持有会话的输入控制器当场放手（macOS 输入法自己的原生词库窗口用）。announce 可以为 NULL。 */
char *msime_client_settings_dictionary_announced(const uint8_t *request, size_t length,
                                                 void (*announce)(void *context), void *context);
/* Smart punctuation follow-up gestures. The host holds the snapshots: they belong
 * to its editor, not to Engine, and a session rebuilt while the keyboard was away
 * must not carry a gesture across the gap. The switches that gate them live in the
 * applied preferences, so the session answers rather than the host keeping a copy.
 * arm: {ascii, commit, timestamp_ms, editor_generation, auto_closed_pair} ->
 *      {repeat: {ascii, committed, timestamp_ms, editor_generation}|null,
 *       space: {chinese, ascii, editor_generation}|null}
 * decide: {character, preceding, timestamp_ms, editor_generation, repeat, space} ->
 *      {replace_with: "，"|null, space_ascii: 46|null}
 * `preceding` is what the editor holds before the caret at the moment of the press;
 * both decisions re-read it and decline when it disagrees with the arming, so a
 * stale snapshot can never rewrite the wrong character. A non-null space_ascii means
 * replace the preceding mark with it and swallow the space. Maximum 4096 bytes. */
char *msime_client_smart_punctuation_arm(uint64_t handle, const uint8_t *request, size_t length);
char *msime_client_smart_punctuation_decide(uint64_t handle, const uint8_t *request, size_t length);
/* Pure Engine validation/normalization for one Entry object. No paths or
 * session are required and no dictionary state is changed. */
char *msime_client_dictionary_validate(const uint8_t *request, size_t length);
/* Plain Chinese words, one per line, answered as the pinyin entries the "hans" import format would produce: {entries:[Entry]}. Reads only the packaged main dictionary under resources, read-only; needs no prepared host or session and changes no dictionary state. */
char *msime_client_dictionary_hans_entries(const uint8_t *text, size_t text_length,
                                           const uint8_t *resources, size_t resources_length);
/* 搜狗细胞词库（.scel）转成标准 TSV 文本：请求 {bytes_base64}（文件不超过 8 MiB），回答 {name, category, entries, text}。text 每个词一行（词<TAB>音节'音节，不带权重列），读不出读音或词的那一行必定被导入拒绝，所以导入报告里的行号就是词序；宿主把 text 当作 standard 格式交给自己现有的导入。纯函数：不需要宿主选项，不碰任何词库状态，不记录词条内容。错误：dictionary_scel_unsupported（不是细胞词库或版本不支持）、dictionary_scel_malformed（不完整或已损坏）、dictionary_scel_empty、dictionary_too_large、dictionary_scel_invalid（请求或 base64 坏了）。 */
char *msime_client_dictionary_scel_text(const uint8_t *request, size_t length);
/* A dictionary file ({kind, format, text}, as the "import" dictionary action) answered as the words the personal dictionary queue accepts, with the import report: {entries:[Entry], applied, failed, truncated, swapped, first_failures}. Invalid rows are counted with their line, repeated words appear once, and at most 128 words are returned. Reads only the packaged main dictionary under resources; changes no dictionary state. Maximum 1,200,000 bytes. */
char *msime_client_dictionary_import_entries(const uint8_t *request, size_t request_length,
                                             const uint8_t *resources, size_t resources_length);
/* 个人词库队列同步：请求是与 msime_client_create 相同的 HostOptions。
 * 一次最多应用 4 项，返回 {synchronized,pending_count,failed_count,snapshot_error}。
 * 整批先取得维护租约；词库仍有活跃会话时返回 dictionary maintenance busy，队列保持原样，稍后重试。 */
char *msime_client_personal_dictionary_sync(const uint8_t *request, size_t length);
/* The shared preference defaults as a JSON document. A host patching one key of
 * a nested preference object needs that object's other fields: the object itself
 * is optional, its members are not.
 */
char *msime_client_default_preferences(void);
/* 触控板设置元数据：defaults 为共享默认值，horizontal_dp 和 vertical_dp 各含 minimum / maximum。纯计算，返回标准 JSON 结果，用 string_free 释放。 */
char *msime_client_text_trackpad_options(void);
/* 「重置所有设置」：directory 是绝对的偏好目录（UTF-8，<=16384 字节）。在偏好锁里读出当前文档，换成本目录所属版本的默认偏好（服务凭据与 fuzzy_pinyin.seeded 保留），再按 expected_revision 比较并交换写回，返回新的 PreferencesSnapshot。修订号不符时失败且不写入。词库、统计不受影响；默认关闭剪贴板历史，因此与 msime_client_save_preferences 一样清空已存的历史。会读写文件，请在工作线程调用。 */
char *msime_client_restore_default_preferences(const uint8_t *directory, size_t length,
                                               uint64_t expected_revision);
/* 「导出设置」：directory 是绝对的偏好目录（UTF-8，<=16384 字节）。返回值 value 是设置文件全文（JSON 字符串，format 为 app.msime.client.preferences），不含语音、AI 辅助和翻译服务的配置与密钥、诊断日志、使用统计和剪贴板历史开关。宿主负责让用户选位置并写盘。会读文件，请在工作线程调用。 */
char *msime_client_export_settings(const uint8_t *directory, size_t length);
/* 「导入设置」：document 是用户选的设置文件内容（<=1048576 字节）。换算成偏好（本机的服务配置与密钥、诊断日志、使用统计和剪贴板历史开关保留，按本目录所属的版本收窄输入方案；不会清空剪贴板历史）后按 expected_revision 比较并交换写回，返回新的 PreferencesSnapshot。失败时 error 为 settings_document_invalid（不是设置文件）、settings_document_macos（macOS 原生设置窗口导出的文件）、settings_document_unsupported（更新版本的文件或读不懂的设置）、settings_conflict（设置已在别处更新）之一，或存储错误的说明；失败时不写入。会读写文件，请在工作线程调用。 */
char *msime_client_import_settings(const uint8_t *directory, size_t directory_length,
                                   uint64_t expected_revision, const uint8_t *document,
                                   size_t document_length);
/* The transcription provider and optional rewrite this device is configured for, read from an
 * absolute preferences directory. Response value: {provider:{...}|null, polish:{...}|null}; both
 * absent means nothing is configured and the host uses whatever it falls back to. Contains
 * credentials: never log the response; release with msime_client_string_free. provider.requestFormat 是请求格式："multipart"（OpenAI 兼容的 /audio/transcriptions）、"chat_audio"（Chat Completions 带 input_audio，阿里云百炼）、"doubao_websocket" 或 "local"；宿主按它挑请求构造，不按 provider 名字判断。 */
char *msime_client_mobile_voice_configuration(const uint8_t *directory, size_t length);
/* 全局主题选择器：{themes:[{id,title,platforms,appearance,preview,candidate,keyboard},...],default:"system"}。只列出本库编译到的平台提供的主题，按选择器顺序：system, native（仅 iOS）, shuishan, light, paper, night, ink, custom。platforms 为提供该主题的宿主（如 ["ios"]），null 表示所有宿主。appearance is "light"|"dark"|null; preview {background,panel,accent,text}, candidate and keyboard are the built-in palettes and are null for system, native and custom. Keys are snake_case. Hosts keep no copy of the ids, titles or colours. */
char *msime_client_theme_catalog(void);
/* 解析所选全局主题的颜色。JSON 请求（<=1048576 字节，拒绝未知键）：{global_theme:string, custom_theme?:Preferences.custom_theme, dark:bool, layout:"horizontal"|"vertical", skins_directory?:皮肤根目录的绝对路径 | package?:candidate_skin_catalog 中的一项}。global_theme 必须是八个主题 id 之一（system、native、shuishan、light、paper、night、ink、custom），其他 id 请求失败；native 不在本平台目录里时也接受，解析结果与 system 相同，只是 id 为 native。custom_theme 按偏好同样校验。dark 是宿主当前的明暗模式，它决定自定义主题取哪个槽位：深色模式取 custom_theme.candidate_skin_dark，没设时取 custom_theme.candidate_skin，浅色模式取 custom_theme.candidate_skin。皮肤包只在它清单 base 的明暗下绘制：base 是固定明暗的内置主题（shuishan、night、ink 为深色，light、paper 为浅色）且与 dark 不符时不画，base 为 system 的包两种模式都画；设过皮肤却在这个模式下没有可画的包时，custom_theme.base 也只在属于当前明暗时作底，否则按 system。layout 是正在绘制的候选窗：皮肤包只在清单声明的排列和模式下绘制，只有画了它时 candidate_skin 才非空，所以宿主正好在 candidate_skin 非空时画包的装饰和最小宽度。skins_directory 给扫描皮肤根目录的宿主（除 Linux 外都是）；package 是 Linux candidate_skin_catalog 中当前模式槽位指名的那一项，其他任何东西（包括 msime_client_skin_catalog 的 SkinSummary）都让请求失败。响应 value：{id,source:"system"|"builtin"|"custom",appearance:"light"|"dark"|null, candidate:{surface,border,text,number,secondary,accent,selected,selected_text,selected_number,hover,show_selected_bar}|null, keyboard:{background,key,function_key,text,secondary,accent,on_accent}|null, candidate_skin:string|null}。appearance 非空时是返回的界面所处的明暗。keyboard.accent 是触屏候选条选中候选的文字色（无填充）；回车键保留平台强调色。on_accent 为黑或白，能在强调色填充上读清。每个颜色都是 #RRGGBB 或 #RRGGBBAA。为 null 的调色板或槽位表示用宿主自己的原生 token，从不表示透明。根目录里没有、磁盘上无效、或不是当前槽位指名的包会被略过而不是让调用失败。skins_directory 会读取皮肤包：在主题、明暗或皮肤包变化时解析，不要在绘制时调用。 */
char *msime_client_resolve_theme(const uint8_t *request, size_t length);
/* 应用主题（Android 本地设置里的 app_theme，不在共享偏好里）选择器：{app_themes:[{id,title,season,seasonal,light:{accent,accent_soft,on_accent,background,card,hair},dark:{...同上}}],default:"siji"}。id 按选择器顺序为 siji、chunya、xiayin、qiushan、dongxue；season 是固定的那一季（"spring"|"summer"|"autumn"|"winter"），siji 为 null 且 seasonal 为 true，它的颜色固定画秋杉，目录因此不随时钟变化。颜色为 #RRGGBB 或 #RRGGBBAA；accent_soft 是强调色加 22（浅色）或 40（深色）透明度，浅色 on_accent 为 #FFFFFF，深色 on_accent 是强调色与黑色 25% 比 75% 的混合。纯计算。 */
char *msime_client_app_theme_catalog(void);
/* 应用主题在宿主当前月份与明暗模式下的颜色。JSON 请求（<=4096 字节，拒绝未知键）{app_theme:"siji"|"chunya"|"xiayin"|"qiushan"|"dongxue", month?:1-12, dark:bool}；month 是宿主本地日历的月份，只影响 siji，省略时用 UTC 月份，超出 1-12 时请求失败。返回 {id, season, accent, accent_soft, on_accent, background, card, hair}，season 是实际画的那一季。纯计算，可在主线程调用。 */
char *msime_client_resolve_app_theme(const uint8_t *request, size_t length);
/* Per-key double-pinyin hint text for one profile name, as a JSON object mapping
 * an uppercase key to "initials / finals" - or to whichever side that key carries.
 * Read out of the Engine's own profile tables so a keyboard face never carries a
 * second copy of the keymap. An unknown profile name yields an empty object
 * rather than the default profile's hints. */
char *msime_client_shuangpin_key_hints(const uint8_t *profile, size_t length);
/* The double-pinyin codes of the whole zero-initial syllables for one profile name, as a JSON object mapping each syllable to its two-key code, e.g. {"a":"aa","ang":"ah",...}. Read out of the Engine's own profile tables. An unknown profile name yields an empty object. */
char *msime_client_shuangpin_zero_initials(const uint8_t *profile, size_t length);
/* 规范化中文触屏快捷符号：{symbols: string[]}，最多 64 KiB。标准响应 value 为去重的单字素列表，最多十二项；无有效项时返回共享默认值。不落盘，用 string_free 释放。 */
char *msime_client_touch_quick_symbols(const uint8_t *value, size_t length);

/* Load PreferencesStore from an absolute UTF-8 directory, without a session.
 * May block on disk/file lock: use a worker thread. Returns PreferencesSnapshot.
 * Missing file returns shared defaults; malformed/future files return errors.
 * Creates the directory/lock file if absent, never overwrites preference contents.
 */
char *msime_client_load_preferences(const uint8_t *directory, size_t length);
/* 可重绑切换热键（keybindings.toggle_english_candidates、toggle_full_width、toggle_punctuation、toggle_voice_input）按共享 KeyChord 契约解码。请求是 JSON {"keybindings": object}（<=65536 字节）；keybindings 缺省或为 null 时按出厂值，不是 object 时返回错误。value 是 {"english_candidates","full_width","punctuation","voice_input"}，每项是 null（写法为空或不合法：不绑定）或 {"key": 主键规范名, "ctrl": bool, "alt": bool, "shift": bool, "meta": bool}；规范名是 client-core KeyChord 的 Display 输出（KeyA-KeyZ、Digit0-Digit9、F1-F12、Space、Semicolon、Equal、Comma、Minus、Period、Slash、Backquote、BracketLeft、Backslash、BracketRight、Quote），宿主按规范名认键，写法的解析只在 Rust 一侧。结果用 msime_client_string_free 释放。 */
char *msime_client_toggle_hotkey_chords(const uint8_t *request, size_t length);
// 录键试写：{"field": 槽位, "chord": 写法, "keybindings": 偏好, "voice_ctrl_f9": bool}。
// value 为 {"ok": true, "chord": 规范写法}，或 {"ok": false, "kind": "invalid"|"conflict", "owner"?: 占用者}。
// 请求最多 64 KiB，不落盘；返回值通过 msime_client_string_free 释放。
char *msime_client_check_toggle_hotkey(const uint8_t *request, size_t length);
/* 私有的聚合打字统计。JSON 请求（<=65536 字节）：
 * {directory:绝对路径,action:{operation:"load"|"reset"}}
 * {directory,action:{operation:"set_enabled",enabled:bool}}
 * {directory,action:{operation:"set_retention",retention,day:"YYYY-MM-DD"}}
 *   retention 取 forever|30d|90d|180d|365d，其他值一律按 forever 读，绝不当成更短的时间窗。`day` 是调用方的本地日期。
 * {directory,action:{operation:"record",text,source,day:"YYYY-MM-DD",hour?:0-23}}。
 * record 在内存里给上屏文本分类，只持久化聚合计数；文本既不返回也不存储。可能阻塞在磁盘或文件锁上：请在工作线程调用。`hour` 是这次上屏的本地小时，必须与 `day` 取自同一时刻；拿不准就省略，不要猜，这一天的计数照样保留，只是不按小时拆分。record 返回 {recorded, milestone}：milestone 是总数刚刚越过的成就计数（100、1000……），没有越过时为 null。只有本进程里有会话开着成就音效时才计算它，而在桌面宿主上此时提示音已经排进队列。
 * {directory,action:{operation:"record_keys",day:"YYYY-MM-DD",keys:{"KeyA":3,...}}}
 *   把各键的按键次数加到 `day` 上，`day` 是这些按键发生的本地日期（跨过午夜的一批先按旧日期写出）。每个键只存每日按键次数：不存顺序、时间或文本。键 id 是 W3C KeyboardEvent.code 名加上软键盘 id（Nine0-Nine9、SoftPunctuation、SoftSymbol、SoftLayer、SoftLanguage、SoftGlobe、SoftEmoji、SoftVoice）；完整列表是 client-services typing_statistics.rs 里的 KEY_IDS。只要有一个未知 id 或零计数就拒绝整批；绝不要自造 id。返回 {recorded:n}，统计关闭时为 0（什么都不写）。宿主在内存里攒批，从工作线程调用，绝不每按一个键调用一次。
 * {directory,action:{operation:"summary",day:"YYYY-MM-DD",user_words?:n}} 返回调用方本地 `day` 的派生指标（总览、习惯、按键、成就）；统计开着时，新解锁的成就在锁内写下。user_words 是词库 "count" 加 user_only:true 的结果。隐私模式不影响它：它只读。
 * {directory,action:{operation:"record_voice",day,milliseconds}} 记一次语音输入的时长；{directory,action:{operation:"record_skin",id}} 记一次用到的皮肤。两者都返回 {recorded}。
 * 「上传到云端」，给自己发 HTTP 的宿主（Android、iOS、鸿蒙）：开关、设备标识和上次上传的时间存在 directory 下的 typing-statistics-upload.json 里，构建和十分钟的节流都在这里，宿主只负责发请求。{directory,action:{operation:"upload_status"}} → {enabled,lastUploadMs}（lastUploadMs 还没有上传过时为 null）。{directory,action:{operation:"set_upload_enabled",enabled}} → 同上：只改本机状态、不联网；打开时第一次生成设备标识，关闭时清掉上次上传的时间（设备标识保留，重新打开时服务端仍认作同一台设备）。宿主打开前要求用户已登录，打开后接着做一次 force 的 upload_request；关闭后自己发 DELETE /v1/users/me/typing-statistics 删掉云端副本，删除失败时开关仍是关的。{directory,action:{operation:"upload_request",day,platform,force?}} → 要发时 {method:"PUT",path:"/v1/users/me/typing-statistics",device,body}，body 是请求体对象（{device,platform,days:[{day,characters,active_seconds,hours?,categories?,sources?}],selections?}），宿主带登录账号的令牌、以 application/json 发出；不发时 {skip}，取 disabled（没打开上传）、not_due（离上次上传成功不到十分钟，force 为 true 时不看）或 nothing（最近 60 天没有字数）。day 是宿主的本地日期，platform 是 android、ios、harmony 等服务端认的平台 id（不认识的平台发出去是空串）。这一步不记任何东西：发送失败时什么也不用做，下一次照样交回请求。{directory,action:{operation:"upload_succeeded",device}} → {enabled,lastUploadMs}：服务端回 204 以后调用，device 是上一步交回的 device；期间用户关掉了上传或设备标识变了时不记。宿主要把 upload_request、发送、upload_succeeded 和关闭后的 DELETE 放在同一个串行队列里，免得关掉后的删除和还在路上的上传交错。
 * 请求顶层可加 status:true（只用于 load、set_enabled、set_retention 和 reset，用于别的操作时整个请求被拒绝）：回答换成设置页读的状态 {statistics,availability:"ready"|"neverWritten",lastWrittenMs:n|null}，availability 和 lastWrittenMs 来自统计文件的修改时间，与 Tauri 外壳的 load_typing_statistics 相同。
 */
char *msime_client_typing_statistics(const uint8_t *request, size_t length);
/* Read only the aggregate-statistics master switch from an absolute UTF-8
 * directory. Returns 1 when enabled, 0 when disabled/missing, and -1 for an
 * invalid directory or unreadable document. Intended for native capture gates:
 * call on activation or a settings-change notification, never per keystroke. */
int32_t msime_client_typing_statistics_enabled(const uint8_t *directory, size_t length);
/* Scan an absolute UTF-8 skin root and return the catalog the settings page sees: {packages:[...],issues:[...]}. Reads the directory: use a worker. An unreadable root is an empty catalog; an invalid package becomes an issue and is never returned as renderable. Presenters must still check that a package supports the layout and theme before adopting its colors. Keys are camelCase, the same document the settings page consumes. Besides the colours (drawn through msime_client_resolve_theme, where a palette's translation becomes secondary) a package carries what a presenter draws itself: minWidthDip; cornerRadiusDip (0-32, null keeps the host radius); decorationTopDip, decorationWidthDip and decorationImage (package-relative, null unless decorated) placed by decorationAlign ("left"|"center"|"right"), with decorationMotion ("none"|"float"|"sway"|"breathe"|"bounce", an idle motion of the whole image while the window is on screen), decorationMotionPeriodMs (1200-8000) and decorationFrames (0 without a decoration image, 1 for a still image, the animation frame count of an APNG, GIF or WebP otherwise; a host that cannot play frames draws the first); background null or {image,fit:"cover"|"contain"|"stretch",opacity:0-1}, drawn over the surface and under the candidates, clipped to the card outline; toolbar {cornerRadiusDip|null, light, dark} where each mode is {background,border,handle,divider,icon,hover}, each #RRGGBB, #RRGGBBAA or null for the host's own. A package may also carry notices, non-fatal strings about manifest keys it parsed but does not apply here (canvas-only keys on a non-canvas base, or a canvas package's nine-slice panel and widgets, which no msime host draws); the key is absent when there are none and the package stays usable. Image paths are already confined to the package and are images; read them with msime_client_skin_resource. */
char *msime_client_skin_catalog(const uint8_t *directory, size_t length);
/* JSON {directory:absolute skin root,id:package folder}. Validates that one package with the same loader as msime_client_skin_catalog and returns one of its camelCase packages entries; an invalid, built-in, symlinked or missing package is {ok:false,error} with the loader's reason. Reads the package: resolve on a skin or appearance change, never while drawing. */
char *msime_client_skin_package(const uint8_t *request, size_t length);

/* 图片工具栏的无 I/O 布局。请求 {toolbar: 扫描缓存中的 toolbarImage,
   shown: [[按钮键, 当前状态], ...]}，最多 7 个当前显示按钮；皮肤只能进一步隐藏。
   返回 {widthDip,heightDip,buttons:[{key,xDip,yDip,widthDip,heightDip,iconSizeDip,
   background,image}]}，尺寸覆盖背景与全部按钮。失败不能继续使用旧命中区。
   UTF-8 JSON 最多 64 KiB；结果用 msime_client_string_free 释放。 */
char *msime_client_skin_toolbar_image_layout(const uint8_t *request, size_t length);
/* 同步解码有界内存中的 PNG/APNG 默认图或 GIF/JPEG/WebP 首帧，不读文件。
 * 输入最多 8 MiB，边长最多 2048。成功时只调用一次回调，rgba 为非预乘 RGBA8，
 * length = width * height * 4。字节只在回调期间有效，宿主必须当场复制；回调不得抛异常。
 * 只有回调返回 1 时整个调用返回 1；其他回调值返回 0。
 * 坏图、空指针和超限返回 0，且不调用回调。
 * 只能在皮肤资源变化时加载，不能在逐键绘制中解码；这不是动画播放接口。 */
typedef int32_t (*msime_client_skin_image_callback)(void *context, const uint8_t *rgba,
                                                   size_t length, uint32_t width, uint32_t height);
int32_t msime_client_skin_image_decode(const uint8_t *bytes, size_t length,
                                      msime_client_skin_image_callback on_frame, void *context);
/* 同步解码整张 PNG/APNG、GIF、JPEG、WebP，全部验证成功后才按序回调。
 * 每帧为完整画布，像素格式、借用期和线程要求与单帧接口一致；最多 120 帧、800 万像素。
 * index 从 0 开始，count 为总帧数；delay_ms 是该帧持续时间，静态图为 0。
 * play_count 是完整播放次数，0 表示无限；有限播放结束保持末帧。
 * GIF 无循环声明时播放一次；APNG/GIF <=10ms 归一化为 100ms；WebP 多帧固定 100ms 无限。
 * 坏图、空指针和超限返回 0 且无回调。任一回调不返回 1 时停止并返回 0，宿主须丢弃已收帧。
 * 宿主必须对同一包的不同图片累计像素预算；不能在逐键绘制中调用。 */
typedef int32_t (*msime_client_skin_animation_callback)(void *context, const uint8_t *rgba,
    size_t length, uint32_t width, uint32_t height, uint32_t index, uint32_t count,
    uint32_t delay_ms, uint32_t play_count);
int32_t msime_client_skin_animation_decode(const uint8_t *bytes, size_t length,
    msime_client_skin_animation_callback on_frame, void *context);
/* JSON {directory:absolute path,id:skin id,relative:package asset,kind:"image"|"font"}.
 * Returns {contentType,bytes}; the manifest and package containment are
 * revalidated for every call and the requested kind must match the asset.
 * With the optional encoding:"base64" it returns {contentType,bytesBase64} instead: the bytes array costs about 64 bytes of heap per resource byte while the response is built, so every msime caller asks for base64. Any other encoding is an error. */
char *msime_client_skin_resource(const uint8_t *request, size_t length);
/* JSON {directory:absolute path,id:skin id}; returns a nullable stylesheet
 * string from the manifest, after revalidating the package and path. */
char *msime_client_skin_toolbar_stylesheet(const uint8_t *request, size_t length);
/* JSON {source:absolute picked folder,directory:absolute skin root}. Copies the
 * folder under its own name, replacing a skin of that name whole; returns {id}.
 * The error message is skin_name, skin_manifest or storage. Touches the disk. */
char *msime_client_skin_import(const uint8_t *request, size_t length);
/* The queued personal dictionary, for a host that cannot take the Engine's
 * maintenance lock when the request arrives. Same request shape as
 * msime_client_dictionary - {options,action} - but the operations act on
 * <preferences_directory>/PersonalDictionary instead of the Engine, and the
 * keyboard applies them at its next session start. Use this for import_personal
 * in particular: "maintenance busy" is not an answer to "add these words", and
 * the user is as likely to import with the keyboard up as with it down. */
char *msime_client_personal_dictionary_request(const uint8_t *request, size_t length);

/* 共用键盘设计操作，不写磁盘。请求 operation 为 catalog、reset、template（index）、
 * background（index、design）或 optimize_contrast（design）。返回普通 {ok,value} JSON；
 * catalog 返回 default、templates、backgrounds，其余返回完整设计。未知字段、无效设计、
 * 越界索引和超过 1 MiB 的请求返回 keyboard_skin_invalid。结果由 msime_client_string_free 释放。 */
char *msime_client_keyboard_skin_design(const uint8_t *request, size_t length);
/* Absolute staged engine resources; returns {profile,sourceCommit} from the
 * packaged msime-dictionary-manifest.json. Two fields only: the page is asking what
 * dictionary is installed and where it came from, not for journal modes or
 * third-party references. Missing or unreadable is reported, never guessed -
 * showing the wrong dictionary version is worse than showing none. */
char *msime_client_dictionary_manifest(const uint8_t *resources, size_t length);
/* Read saved history only; disabled preferences return an empty entries array. */
char *msime_client_load_clipboard_history(const uint8_t *directory, size_t length);
/* JSON {directory,text}; removes exact saved entry, not the system clipboard. */
char *msime_client_remove_clipboard_history(const uint8_t *request, size_t length);
/* JSON {directory,text}; capture under the shared preference/history locks. */
char *msime_client_capture_clipboard_history(const uint8_t *request, size_t length);
/* 桌面剪贴板历史，供「剪贴板」设置页使用：与 Tauri 外壳的剪贴板命令读写同一个 directory/clipboard_history.json。request 是 length 字节（最多 131072）的 UTF-8 JSON {directory: 偏好目录的绝对路径, action}，action 是 {operation:"list"|"sync"|"clear"}、{operation:"set_pinned",text,pinned} 或 {operation:"remove",text}；text 按内容找条目，非空且不超过 12000 字节。sync 只在 clipboard_history 开着时读系统剪贴板（只有 Windows 读，带密码管理器等隐私标记的内容不记录，末尾换行去掉），关着时 error 是 clipboard_history_disabled，其他平台读不到时是 clipboard_history_unavailable。clear、set_pinned 和 remove 不看开关。每个操作都返回 {enabled, entries:[{text,timestampMs,pinned}]}，是操作后的历史，先固定的、再按时间从新到旧；开关关着时 entries 为空。请求不合法时 error 是 clipboard_history_invalid，读写失败时是 clipboard_history_unavailable。读写磁盘、sync 还要打开剪贴板：放在工作线程上调用。 */
char *msime_client_desktop_clipboard_history(const uint8_t *request, size_t length);
/* 移动端的结构化剪贴板历史，与桌面的自动记录偏好无关。请求 JSON 为 {directory: 历史根目录的绝对路径, preferences_directory?: 偏好目录的绝对路径, action}；省略 preferences_directory 时从 directory 读取偏好，供原有宿主继续使用。action 是 {operation:"load"|"clear"}、{operation:"capture"|"remove",text}、{operation:"remove_many",texts}、{operation:"restore_many",entries}、{operation:"set_pinned",text,pinned} 或 {operation:"replace",text,replacement}。remove_many 原子返回 removed 完整条目，restore_many 原子恢复其文字、时间戳和固定状态；已有同文条目不覆盖。replace 按内容找到原条目原地改字，时间戳和固定状态不变；新文字与另一条相同时两条合并，固定状态取两者之或；返回 replaced、merged 和 reason（"not_found"|"invalid"|null）。历史存在 directory/MSIME/clipboard_history.json。 */
char *msime_client_mobile_clipboard_history(const uint8_t *request, size_t length);
/* Same validation as load_preferences; ok:true,value:null means lock busy.
 * Does not wait for the writer lock. Disk I/O may still block: use a worker.
 * Busy is not missing/corrupt and must not reset preferences to defaults. */
char *msime_client_try_load_preferences(const uint8_t *directory, size_t length);
/* Repair a config.json that is not well-formed JSON (truncated, empty, overwritten). May block on disk and the writer lock: use a worker. Backup first: the damaged bytes are copied verbatim to <directory>/config.json.corrupt-YYYYMMDD-HHMMSS (UTC, -N suffix when taken) and nothing is rewritten if that copy fails. Then every setting and service key the schema still accepts is carried onto the defaults. A valid or missing document is a no-op: {recovered:false, snapshot}. A repair returns {recovered:true, snapshot, backup_path, backup_name, salvaged}. A well-formed document the schema rejects (unknown fields, newer format_version) is most likely a newer build's and returns the load error unchanged; the settings page repairs it explicitly with msime_client_repair_preferences. Storage errors are returned and never lead to a rewrite. */
char *msime_client_recover_preferences(const uint8_t *directory, size_t length);
/* The settings page's explicit 修复配置文件: the same repair for any document load_preferences rejects, a newer build's (unknown fields, newer format_version) included. Same backup, salvage, result shape and worker requirement as msime_client_recover_preferences. Call it only when the user asks: what this schema does not accept takes its default. */
char *msime_client_repair_preferences(const uint8_t *directory, size_t length);
/* Compare-and-swap save of PreferencesSnapshot.preferences. The snapshot's
 * format_version is validated; expected_revision must match the store.
 * A disabled clipboard-history save clears the default history file if history
 * is still disabled. Cleanup errors may be returned after preferences are saved.
 * Directory <=16384 bytes; snapshot <=1048576 bytes, enough for a custom skin photo. */
char *msime_client_save_preferences(const uint8_t *directory, size_t directory_length,
                                    uint64_t expected_revision,
                                    const uint8_t *snapshot, size_t snapshot_length);
/* 在会话线程传入 PreferencesSnapshot JSON（最多 1048576 字节）：{format_version:1, revision, preferences:{...}}。修订号按会话递增；相同快照可重试，旧版或冲突快照会被拒绝。返回 {revision, deferred, view, diagnostic?}；仅首选方案无法运行并回退时带 diagnostic。组字期间的 Engine 偏好应用延后到派发或焦点调用使会话空闲，但关闭云候选会立即清除现有云行和旧查询，AI 配置或令牌变化也会立即清除旧 AI 行和查询；释义开关、目标语言或翻译服务变化会清掉旧释义并推进候选代次，旧释义回包将被拒绝。宿主应使用返回的新 view。较新快照替换待应用快照。构建失败时保留旧会话和待应用快照供重试，派发通过 diagnostic 报告重试错误，不丢失已完成输入。本接口不读写偏好文件，宿主提供已加载的快照。 */
char *msime_client_update_preferences(uint64_t session, const uint8_t *snapshot, size_t length);
/* 获得焦点（true）只重置旧组字，包括 phrase_preedit 暂存的词段，不返回旧组字的提交；失焦（false）按当前方案结束组字，并返回应送出的提交，包括已经选好的暂存词段。 */
char *msime_client_focus(uint64_t session, bool focused);
/* Clear the current session's Engine candidate cache and refresh its view. */
char *msime_client_reset_cache(uint64_t session);
/* Release process-level model caches without a session handle (memory pressure). */
void msime_client_release_process_cache(void);
/* 语音设置服务目录。纯函数，无文件、网络或设备访问；响应 value 为 {asr:[preset],polish:[preset],doubao_stream_endpoints:[{id,label,title,endpoint}]}。
 * preset 为 {id,title,endpoint,model,models:[string],documentation}，仅含公开预设。
 * 数组顺序即设置页顺序；不含平台特有的 system/local，宿主按能力追加。
 * models 仅作建议，不限制用户自定义模型。宿主复制字段后用 msime_client_string_free 释放返回的 JSON 缓冲区。 */
char *msime_client_voice_provider_catalog(void);
/* 设置页识别服务行的状态文案。provider 为 system、local、云端服务 id 或空串；resolved
 * 表示共享解析已得到可用云端配置，platform_available 是宿主是否有系统识别服务，后两个
 * 参数只在 provider 为 local 时使用。纯计算，返回 {status:string|null}。 */
char *msime_client_mobile_voice_status(const uint8_t *provider, size_t provider_length,
                                       bool resolved, bool platform_available,
                                       bool model_ready, bool runtime_ready);
/* 设置页润色行的状态文案；enabled 是润色开关，configured 表示共享解析已有可用配置，
 * has_token 表示当前服务或槽位存在凭据。纯计算，返回 {status:string|null}。 */
char *msime_client_mobile_voice_polish_status(bool enabled, bool configured, bool has_token);
/* Android 语音设置的服务分类和豆包鉴权模式。provider 与 auth_mode 是 <=256 字节 UTF-8，返回
 * {network:boolean,legacy_auth:boolean}，纯计算，不读取偏好或设备状态。 */
char *msime_client_mobile_voice_policy(const uint8_t *provider, size_t provider_length,
                                       const uint8_t *auth_mode, size_t auth_mode_length);
/* 切换语音服务的纯函数；请求 <=1 MiB：{service:"voice.asr"|"voice.polish",provider:string,
 * current:{provider?,endpoint?,model?,token?,tokens?:{id:string}},field_policy?:"fill_defaults"|"clear"}。
 * current 缺少的字段按空值处理。field_policy 缺省为 fill_defaults，只替换空白或出厂值；clear 在换服务时清空地址和模型。
 * value 为 {provider,token,tokens,endpoint?,model?}：省略地址/模型表示保留原值，空串表示清空。
 * 不读写偏好、不联网；宿主负责把补丁字段映射到 asr_ 或 polish_ 并保存。
 * 请求和返回含凭据，禁止记录；返回缓冲区用 msime_client_string_free 释放。 */
char *msime_client_voice_provider_update(const uint8_t *request, size_t length);
/* 语音设置接口预检；输入是 <=64 KiB 的 UTF-8 地址，websocket 为 true 时按 wss 校验，否则按 https 校验。
 * 首尾空白与运行时解析一样去除，空地址表示使用默认值；非空地址复用运行时的 2048 字节和 URL 形状规则，并保留移动宿主的小写协议、无 @ 限制。
 * value 为 {accepted:boolean,issue:string|null}；失败提示固定，不回显输入，不访问偏好或网络。
 * 空引用、超限和非法 UTF-8 返回错误；返回缓冲区用 msime_client_string_free 释放。 */
char *msime_client_voice_endpoint_issue(const uint8_t *endpoint, size_t length, bool websocket);
/* 纯语音凭据预检。空值或空白值表示删除并接受；其他值若含控制字符、超过 16 KiB，或豆包握手凭据不是 ASCII 则拒绝。
 * 返回 {accepted}，绝不回显凭据。 */
char *msime_client_voice_credential_issue(const uint8_t *provider, size_t provider_length,
                                          const uint8_t *value, size_t value_length);
/* 流式 wss 地址的传输描述；输入是 <=64 KiB 的 UTF-8 地址，实际地址遵循共享 2048 字节与 URL 形状规则。
 * value 为 {socket_host:string,port:uint16,host_header:string,request_target:string}；保留显式端口 0，缺省端口为 443。
 * socket_host 不含 IPv6 方括号，host_header 保留方括号与非默认端口，request_target 使用 URL 库序列化的路径与查询。
 * 不访问网络，失败不回显输入；返回缓冲区用 msime_client_string_free 释放。 */
char *msime_client_voice_websocket_endpoint(const uint8_t *endpoint, size_t length);
char *msime_client_voice_start(uint64_t session);
char *msime_client_voice_cancel(uint64_t session);
/* Capture bounded mono 16 kHz samples. The JSON result is transient audio and
 * must never be logged or persisted. */
char *msime_client_voice_capture(uint32_t milliseconds);
/* The microphones a settings page offers under 「可用录音设备」 (the Tauri settings shell's
 * list_voice_capture_devices on Windows). Value [{backend:"windows",id,label}], at most 128,
 * [] when there is none: id is "wasapi:" then the WASAPI endpoint id as four lowercase hex
 * digits per UTF-16 unit, the value voice_input.capture_device holds with capture_backend
 * "windows". Other targets fail with "voice capture devices unavailable". Enumerates through
 * COM: call off the UI thread. Ids and labels may identify the user's hardware: never log them. */
char *msime_client_voice_capture_devices(void);
char *msime_client_voice_apply(uint64_t session, uint64_t generation,
                               const uint8_t *text, size_t length);
/* On-device speech models and user-dictionary hotwords. JSON request buffers of length bytes; standard responses. Error text of the model calls is a stable code beginning with "local_model_".
 * voice_hotwords: {options: HostOptions as msime_client_dictionary, limit?: 200} -> {hotwords:[{text,pinyin}]}, the user's own pinyin words (two or more Chinese characters), heaviest first. Worker thread; fails with "dictionary maintenance busy" while maintenance holds the store. <=1 MiB.
 * voice_hotword_correct: {text, hotwords:[{text,pinyin}]} -> {text}. Pinyin-similarity replacement for models whose msime-model.json has "hotwords":"pinyin". Pure. The transcript is limited to 10000 Unicode scalar values, with at most 1000 hotwords; each hotword text is <=256 bytes and pinyin <=1024 bytes. The JSON request is <=1048576 bytes.
 * voice_local_models: {root: absolute dir} -> {models:[{id,title,description,languages,streaming,default,desktop_only,installed,path,installed_size,archive_size,memory,license_spdx,license_source,license_terms,license_notice,hotwords,import_files:[{name,url,size}]}], default: id}. path is <root>/<id>, the value for voice_input.asr_model_path. import_files 是不联网安装时要用户自己下载的文件（压缩包和带地址的附加文件），url 是上游下载地址。
 * voice_local_model_install: {root, id, mirror?: "https://..." prefix, files?: [absolute path]} -> {path}。阻塞到安装结束，只能在工作线程调用。progress 可为空，在调用线程收到 {id,stage:"download"|"import"|"verify"|"extract"|"done",downloaded,total}，返回前需复制缓冲区。登记后、访问文件或网络前先回调 download（导入为 import）的零字节进度，total 为目录里的归档字节数；回调可重入 voice_local_model_cancel 取消，本次安装随即报 "local_model_cancelled"。每个 id 同时只允许一次安装（"local_model_install_running"）。带 files（至多 16 个）时不联网，用这些本地文件安装：按长度和 SHA-256 认文件、不看文件名，进度阶段 download 报成 "import"，缺文件时失败为 "local_model_import_missing: <name>"，所选文件打不开或读出错时为 "local_model_import_unreadable: <detail>"。
 * voice_local_model_cancel: {id} cancels that install, NULL/0 or {} cancels all; value is whether one was running. Any thread.
 * voice_local_model_remove: {root, id} -> null. Only catalog ids; refused while that id is installing.
 * voice_model_setting_check: {asr_model_mirror?, asr_model_path?} (<=64 KiB) -> {asr_model_mirror?: bool, asr_model_path?: bool}, one key per field asked: whether the preferences store accepts that value (the same checks as saving). Pure. */
typedef void (*msime_client_voice_local_model_progress_callback)(const uint8_t *json, size_t length,
                                                                 void *context);
char *msime_client_voice_hotwords(const uint8_t *request, size_t length);
char *msime_client_voice_hotword_correct(const uint8_t *request, size_t length);
/* voice_normalize_numbers: {text, kind} -> {text}. 本地模型最终文本里汉字写的数字改成数字（二零零二年 → 2002年，百分之五十七 → 57%），一个、十分有趣这类词不动。只改写不自带 ITN 的 kind（online_transducer、offline_transducer），其他 kind 原样返回，宿主对每个本地模型的最终文本都可以调用；只用于最终文本，不用于中间结果。Pure. text 上限 10000 个 Unicode 标量，kind 上限 64 字节，请求 <=1048576 字节。 */
char *msime_client_voice_normalize_numbers(const uint8_t *request, size_t length);
char *msime_client_voice_local_models(const uint8_t *request, size_t length);
char *msime_client_voice_local_model_install(
    const uint8_t *request, size_t length,
    msime_client_voice_local_model_progress_callback progress, void *context);
char *msime_client_voice_local_model_cancel(const uint8_t *request, size_t length);
char *msime_client_voice_local_model_remove(const uint8_t *request, size_t length);
char *msime_client_voice_model_setting_check(const uint8_t *request, size_t length);
/* 确定性语音后处理（无网络、无状态纯函数）。请求 <=1 MiB：{text, model_kind?, audio_ms?, mode?, app_category?,
 * enable_pangu_spacing?, enable_number_itn?, enable_tech_casing?, trailing_period_policy?, vocabulary?}。
 * text 上限 10000 Unicode 标量，vocabulary 至多 1000 条。响应 value：{text, dismissed, dismissed_reason, changed}。
 * dismissed 为 true 时宿主应静默丢弃而不上屏；返回字符串用 msime_client_string_free 释放。 */
char *msime_client_voice_postprocess(const uint8_t *request, size_t length);
/* 跨语种个人语音词表聚合（只读磁盘与用户词库，不加文件锁，建议在工作线程调用）。
 * 请求 <=1 MiB：{options?: HostOptions, preferences_directory?: 绝对路径, cached_terms?: [...], limit?: 300, raw_text?: string, app_category?: string, prompt_limit?: 60}。
 * 传 cached_terms 时仅做纯内存 prompt_hints 筛选；否则合并用户词库（pinyin/english/quick_phrase，单类不可用时局部降级）与 <preferences_directory>/voice-vocabulary.json；
 * 响应 value：{version, terms:[{canonical,aliases,pinyin,category,weight}], prompt_hints?:[string]}（prompt_hints 至多 60 条）。 */
char *msime_client_voice_vocabulary(const uint8_t *request, size_t length);
/* 管理 <preferences_directory>/voice-vocabulary.json（list / upsert / remove / restore / learn）。
 * 请求 <=256 KiB；采用临时文件原子替换落盘，不加文件锁，应在工作线程调用。 */
char *msime_client_voice_vocabulary_manage(const uint8_t *request, size_t length);
/* LLM 润色/改写/翻译输出清洗与防幻觉门控（纯函数）。请求 <=1 MiB：{raw_text, polished_text, selected_text?, mode?,
 * expect_action_tag?, app_category?, enable_pangu_spacing?, enforce_simplified_chinese?, trailing_period_policy?, vocabulary?}。
 * 响应 value：{accepted, text, resolved_action:"replace_selection"|"insert", rejection_reason}；accepted 为 false 时宿主应降级到确定性后处理原文。 */
char *msime_client_voice_polish_clean(const uint8_t *request, size_t length);
/* Sans-IO 语音润色 HTTP 请求描述器（不执行网络 I/O）。请求 <=256 KiB：{provider, endpoint, model, token?, prompt_id?,
 * custom_prompts?, mode?, enable_selection_rewrite?, target_language?, stream?, raw_text, context?, vocabulary_hints?}。
 * 密码框下非本机回环端点（云端与局域网）返回错误 "polish_secure_field_blocked"，本机回环端点清空上下文、选区与词表后放行；
 * 公网明文地址返回 "polish_invalid_endpoint"，公网缺凭据返回 "polish_missing_token"。
 * 响应 value：{url, method, headers, body, timeout_ms, connect_timeout_ms, max_response_bytes, no_proxy, resolved_mode, expect_action_tag, app_category, stream}。
 * 描述器可能含凭据，严禁写日志，宿主须禁用重定向并按 no_proxy 绕过系统代理。 */
char *msime_client_voice_polish_http_request(const uint8_t *request, size_t length);
/* 解析语音润色非流式 JSON 响应体（<=1 MiB）或 SSE 流式数据块（<=64 KiB）。
 * 响应 value：{text, finish_reason, done}；错误码为 "polish_response_too_large"、"polish_provider_error" 或 "polish_invalid_response"。 */
char *msime_client_voice_polish_parse_response(const uint8_t *body, size_t length, bool is_sse);
/* 语音存档：每次录音的音频（16 kHz 单声道）与听写记录，用宿主交来的 32 字节密钥以 XChaCha20-Poly1305 加密后存在 directory 下，
 * 格式见 client-providers 的 voice::archive。进程里只有一个存档、至多一条正在录的录音；append 可在采集队列上调用，其余在主线程。
 * 返回 JSON 用 msime_client_string_free 释放。错误为 "voice_archive_closed"、"voice_archive_idle"、"voice_archive_invalid_id"、
 * "voice_archive_not_found"、"voice_archive_corrupt"（密钥不对或文件损坏）、"voice_archive_recording"、"voice_archive_io" 或 "invalid voice archive request"。
 * open(directory 绝对路径, key 32 字节) -> {opened}，正在录的录音不受影响。
 * begin {now_ms, app, provider} -> {id}，上一条没停时先替它收尾。append 追加 16 kHz 单声道 f32 样本（一次至多 160000 个），没有正在录的录音时返回 false。
 * end_capture(now_ms) -> {id|null}。note {kind:"text"|"failure"|"cancelled"|"retranscription"|"dismissed", text, id?,
 *   raw_text?, asr_ms?, polish_ms?, mode?, app_category?, polish_provider?, polish_model?, polish_prompt_id?, polish_failed?, polish_rejected_reason?}，
 *   不给 id 时记到最近一次录音；retranscription 必须给 id；dismissed 记录静默丢弃原因而不记作失败。
 * list -> {entries:[{id, started_at_ms, capture_ended_at_ms, audio_ms, app, provider, segments, failure, cancelled, retranscription,
 *   raw_text?, asr_ms?, polish_ms?, mode?, app_category?, polish_provider?, polish_model?, polish_prompt_id?, polish_failed?, polish_rejected_reason?, dismissed_reason?,
 *   status:"recording"|"recognized"|"failed"|"cancelled"|"interrupted"|"no_result"|"dismissed", has_audio}], unreadable}，新的在前。
 * audio(id, format 0=WAV/1=f32 PCM) 把解密后的字节交给 callback 一次（缓冲区在回调返回后失效，不落盘）-> {bytes}。
 * delete(id) -> {deleted}，正在录的那一条不能删。purge {now_ms, recording_retention_hours: 24|72, history_retention_days: 0..36500, 0 为不限期} -> {audio_removed, records_removed}。 */
typedef void (*msime_client_voice_archive_bytes_callback)(const uint8_t *bytes, size_t length, void *context);
char *msime_client_voice_archive_open(const uint8_t *directory, size_t directory_length, const uint8_t *key, size_t key_length);
char *msime_client_voice_archive_begin(const uint8_t *request, size_t length);
bool msime_client_voice_archive_append(const float *samples, size_t count);
char *msime_client_voice_archive_end_capture(uint64_t now_ms);
char *msime_client_voice_archive_note(const uint8_t *request, size_t length);
char *msime_client_voice_archive_list(void);
char *msime_client_voice_archive_audio(const uint8_t *id, size_t id_length, uint32_t format,
                                       msime_client_voice_archive_bytes_callback callback, void *context);
char *msime_client_voice_archive_delete(const uint8_t *id, size_t id_length);
char *msime_client_voice_archive_purge(const uint8_t *request, size_t length);
/* 按需资源包存放在 <state_root>/resource-packs/<id>/；state_root 是 HostOptions 的 preferences_directory（Android: files/bootstrap/state），会话下次取得焦点时读取新发布的包。
 * 包 ID：`japanese`、`language-dictionaries`、`handwriting`、`settled-model`、`grammar-model-wanxiang`、`grammar-model-baguwen`、`grammar-model-baguwen-compact`、`offline-glosses`、`voice-runtime`、`wubi`、`voice-runtime-macos`（锁文件固定了重签的 macOS 运行库之后才有）。请求为 `length` 字节的 JSON（<=16384），响应使用标准封装。`install` 和 `adopt` 是阻塞调用，只能放在工作线程。
 * 错误为 "invalid resource pack request"、"invalid state root"、"invalid source"、"resource_pack_unknown"、"resource_pack_busy"（本进程正在安装或收编该包）或 "local_model_*"。
 * resource_packs: {state_root} -> [{id, state:"missing"|"installed"|"outdated", size, schemes:[...], import_files:[{name,url,size}]}]，每包一项；size 为下载字节数，import_files 为离线安装所需文件（voice-runtime 使用归档），url 为上游下载地址。
 * resource_pack_install: {state_root, pack, sources?:["https://mirror/", ...], files?:[absolute path]} -> {path}。
 * sources 是依次尝试的镜像前缀（最多 8 个，跳过空项），之后尝试项目镜像和来源 URL；固定资源按锁中的大小和 SHA-256 校验，语法模型按官方在线目录的大小和 SHA-256 校验。固定资源未完成文件在下次调用时用 HTTP Range 续传。channel 可选 stable、beta、nightly，语法模型优先选同通道条目，缺失时回退。
 * files（最多 16 个绝对路径）表示改从本地安装，按大小和 SHA-256 匹配而非文件名，忽略 sources，下载阶段改报 "import"；缺文件返回 "local_model_import_missing: <name>"，不可读返回 "local_model_import_unreadable: <detail>"。
 * progress 可为空，在调用线程收到 (context, phase "download"|"import"|"verify"|"done", done, total)，阶段字符串仅在回调期间有效。
 * 首次进度在任务登记后、任何文件或网络 I/O 前发送（done=0，total=资源包大小），回调可重入 resource_pack_cancel；取消后返回 "local_model_cancelled"。
 * resource_pack_cancel: pack 是原始 UTF-8 id（不是 JSON）；NULL/0 取消本进程全部资源包安装。可在任何线程调用，立即返回，不触发进度回调。
 * resource_pack_remove: {state_root, pack} -> {removed: bool}。阻塞卸载已发布的资源包，先摘除引擎侧共享映射缓存；未安装、未发布目录和符号链接返回 false，活跃会话仍持有映射导致的删除失败如实返回错误。只能放在工作线程。
 * resource_pack_adopt: {state_root, pack, source: absolute dir} -> {path}。在同一文件系统内把包文件从 source 重命名移入（不复制），按编译锁校验后发布；失败则全部移回，不发布任何内容。已有相同字节的安装时直接返回，不改动 source。 */
typedef void (*msime_client_resource_pack_progress_callback)(void *context, const char *phase,
                                                             uint64_t done, uint64_t total);
char *msime_client_resource_packs(const uint8_t *request, size_t length);
/* 主动检查语法模型目录，不下载：{state_root,channel?} -> [{pack,source_commit,size}]。 */
char *msime_client_resource_pack_check_grammar_updates(const uint8_t *request, size_t length);
char *msime_client_resource_pack_install(const uint8_t *request, size_t length,
                                         msime_client_resource_pack_progress_callback progress,
                                         void *context);
void msime_client_resource_pack_cancel(const uint8_t *pack, size_t length);
char *msime_client_resource_pack_remove(const uint8_t *request, size_t length);
/* 重新校验资源包的全部固定文件，不写入：{state_root, pack} -> {path}。 */
char *msime_client_resource_pack_verify(const uint8_t *request, size_t length);
char *msime_client_resource_pack_adopt(const uint8_t *request, size_t length);
/* 自定义翻译请求描述器（不做网络 I/O）：api.deepl.com / api-free.deepl.com 用 DeepL v2 格式（DeepL-Auth-Key 鉴权、text 数组，没有密钥时报错），其他地址用 DeepLX 兼容格式。Request <=16 KiB:
 * {config:{enabled,endpoint,api_key},text,source_language,target_language}.
 * Returns null if disabled; otherwise {url,method,headers,body,timeout_ms,max_response_bytes}.
 * Descriptor can contain credentials: never log it. Host enforces timeout/size,
 * rejects redirects and checks HTTP status before parsing. Text <=256 scalars. */
char *msime_client_custom_translation_http_request(const uint8_t *request, size_t length);
/* Pure visible-page plan <=64 KiB: {target_language,candidates:[{text,source}]}.
 * Returns [{text,key,source_language,target_language}]; at most nine candidates. */
char *msime_client_custom_translation_plan(const uint8_t *request, size_t length);
/* Pure signed TMT descriptor <=64 KiB. Input: {config:{enabled,secret_id,
 * secret_key,region},texts:[string],source_language,target_language,timestamp}.
 * Send body_utf8 bytes unchanged. Never log this credential-bearing descriptor. */
char *msime_client_tencent_translation_http_request(const uint8_t *request, size_t length);
/* Pure NiuTrans v2 descriptor <=64 KiB. Input: {config:{enabled,app_id,apikey},
 * text,source_language,target_language,timestamp}. */
char *msime_client_niutrans_translation_http_request(const uint8_t *request, size_t length);
/* OpenAI 兼容的 AI 翻译描述器和解析器。请求不超过 64 KiB：
 * {config: AiAssistantPreferences, input:{text,source_language,target_language}}.
 * It reuses the configured AI endpoint and token, and returns one JSON string or null. */
char *msime_client_ai_translation_http_request(const uint8_t *request, size_t length);
char *msime_client_parse_ai_translation_response(const uint8_t *body, size_t length);
bool msime_client_ai_translation_reply_failed(const uint8_t *body, size_t length);
/* A settings page's 「测试配置」 check, the one the Tauri settings shell runs on Windows and
 * macOS. Request <=64 KiB: {service,config}, service one of translation.tencent ({secret_id,
 * secret_key,region}), translation.niutrans ({app_id,apikey}), translation.custom ({endpoint,
 * api_key}), voice.asr, voice.polish or ai.assistant, config the object the shared page sends.
 * Value {ok,message}: message is the Chinese result line to show; it never carries the
 * provider's reply or the credential. An incomplete config is answered without the network;
 * otherwise this sends one request and blocks up to about 15 s: call off the UI thread.
 * The request carries credentials: never log it. */
char *msime_client_test_api_credential(const uint8_t *request, size_t length);
/* The same check for a settings page without its own copy of the provider defaults (the Android native app). Request <=64 KiB: {preferences_directory, service}, service voice.asr or voice.polish; the configuration is resolved from the stored preferences exactly as the keyboard resolves it. Value {ok,message} as above; a provider with nothing to test (system, local, unknown) is answered without the network. Blocks up to about 15 s: call off the UI thread. */
char *msime_client_test_voice_credential(const uint8_t *request, size_t length);
/* Pure check of a custom translation endpoint (UTF-8, <=64 KiB), for the settings page. Value
 * {issue,accepted}: issue is the Chinese hint the shared page shows under the endpoint field
 * (translationEndpointIssue: empty endpoint, too long, control characters, not a URL,
 * credentials or fragment, remote http), "" when there is none; accepted is whether the
 * preferences store saves it (empty, https, or http to localhost/127.0.0.1/[::1]). No I/O. */
char *msime_client_translation_endpoint_issue(const uint8_t *endpoint, size_t length);
/* Pure check of an AI assistant endpoint (UTF-8, <=64 KiB), for the AI settings page. Value
 * {origin,problem,hint}: origin is the "scheme://host:port" key ai_assistant.tokens stores the
 * token under (aiCredentialOrigin), null when the endpoint is not usable; problem is null,
 * "invalid" or "cleartext_public" (http to a host that is not this machine or the local network);
 * hint is the Chinese line the shared page shows under the field (aiEndpointHint), "" when usable.
 * No I/O. */
char *msime_client_ai_endpoint_check(const uint8_t *endpoint, size_t length);
/* The AI settings page's 「获取模型」, the Tauri settings shell's ai_models. Request <=256 KiB:
 * {endpoint,token}, the token being ai_assistant.tokens[origin]. GETs the /models sibling of the
 * endpoint without following redirects (http only to this machine or the local network, never
 * through a proxy). Value: up to 128 model ids, in the service's order. Error: a stable code,
 * ai_invalid (request or endpoint/token rejected, nothing sent), ai_models_unavailable (no
 * connection, timeout or error status) or ai_models_invalid (reply too large or no ids). Blocks
 * up to about 15 s: call off the UI thread. The request carries the token: never log it. */
char *msime_client_ai_models(const uint8_t *request, size_t length);
/* AI 公开预设数组；含 id/title/endpoint/model/models/documentation/desktop_only/uses_token，不做 I/O。 */
char *msime_client_ai_provider_catalog(void);
/* 纯计算切换补丁。请求 <=16 KiB：`{provider,current:{enabled,provider,endpoint,model,endpoints?,models?}}`。
 * `endpoints`/`models` 是按服务商 `id` 记忆自定义地址/模型的字符串映射，缺省为空映射；切换前保存当前自定义值，再按目标槽位回填，缺键或空字符串使用目录默认值。
 * 返回 `{enabled,provider,endpoint,model,endpoints,models}`；宿主须将两个映射随补丁保存。不传凭据，错误码 `ai_invalid`。不保留请求缓冲区。 */
char *msime_client_ai_provider_update(const uint8_t *request, size_t length);
/* The AI settings page's 「测试」, the Tauri settings shell's ai_test. Request <=256 KiB:
 * {endpoint,model,prompt,token,text}: model non-empty <=256 bytes; prompt (may be empty) and
 * text (non-empty) <=16 KiB each, newlines and tabs allowed. POSTs one non-streaming chat
 * request (prompt as system, text as user) to the endpoint without following redirects. Value:
 * the trimmed reply text. Error: ai_invalid, ai_test_unavailable or ai_test_invalid, as for
 * msime_client_ai_models. Blocks up to about 15 s: call off the UI thread. The request carries
 * the token and the user's text: never log it or the reply. */
char *msime_client_ai_test(const uint8_t *request, size_t length);
// Pure AI descriptor from {config:AI preferences,input:{segmented_pinyin,context,candidate_limit}}.
// Input <=64KiB. Result contains credentials; never log it. Host must forbid redirects.
char *msime_client_ai_http_request(const uint8_t *request, size_t length);
// Successful HTTP body <=1MiB, limit 1..10. Returns a string array or null.
char *msime_client_parse_ai_response(const uint8_t *body, size_t length, uint8_t limit);
// Worker-thread disk I/O. JSON <=64KiB: {directory:absolute private user path,
// action:lookup|remember,target_language,generation,items:[{text,direction,translation?}]}.
// At most 9 items, directions english_to_chinese/chinese_to_english. Remember
// requires translation; lookup forbids it. Returns {generation,translations,saved}.
// Never pass packaged resources. Learned text is private; never log requests.
// A malformed batch makes no writes; an I/O failure can leave earlier items saved.
// New writes use Engine translation-glosses.db; old JSON records remain read-only fallback.
char *msime_client_learned_translation_request(const uint8_t *request, size_t length);
/* Returns [string|null] with exact expected count (1..9), or null for invalid body. */
char *msime_client_parse_tencent_translation_response(const uint8_t *body, size_t length, size_t expected);
/* Provider body <=1 MiB. Returns a formatted translation string or null. */
char *msime_client_parse_niutrans_translation_response(const uint8_t *body, size_t length);
/* Host-produced gloss <=64 KiB UTF-8. Returns it formatted like provider replies (whitespace collapsed, ends trimmed) or null when empty or it has a control char. */
char *msime_client_format_translation_gloss(const uint8_t *text, size_t length);
/* Provider body <=1 MiB. True when the reply reports a failure (malformed, non-200 code, errorCode/errorMsg) rather than an answer, empty or not. Only answers may be negative-cached. */
bool msime_client_custom_translation_reply_failed(const uint8_t *body, size_t length);
bool msime_client_niutrans_translation_reply_failed(const uint8_t *body, size_t length);
/* Provider body <=1 MiB. Returns translation string <=4096 bytes or null when
 * malformed/no result. No session mutation; host validates original identity. */
char *msime_client_parse_custom_translation_response(const uint8_t *body, size_t length);
/* 同代次的非空 JSON [{"text":"candidate","translation":"gloss","gloss":false}] 合并候选释义。
 * [] 主动清空；宿主异步无结果时跳过调用，旧代次不应用。gloss 可省略，缺省为 false；翻译查询 gloss_query 的回答标 true。
 * 候选词翻译范围是「仅当前高亮」时，不带 gloss 标记的回答只画在高亮行（候选的 translation_reserved 为真的那一行），带标记的离线释义每一行都画。 */
char *msime_client_apply_translations(uint64_t session, uint64_t generation,
                                      const uint8_t *translations, size_t length);
/* Resolve copied candidates against the packaged offline English dictionary.
 * JSON request: {generation,candidates:[{text,source}],user_data?,target_language?,state_root?}; the generation is echoed for the host to pass to apply_translations on the session thread. This function owns no session handle and may run on a worker thread.
 * target_language absent or "en" reads msime-english.db and the user's glosses. fr/ja/es/ru/de/ko read only offline-glosses/zh-<lang>.db beside resources, or, when that is absent and state_root (absolute; pass the translation query's state_root) is given, the same file in the downloaded <state_root>/resource-packs/offline-glosses pack; they ignore user_data. When neither is installed the result is {generation,translations:[]}, not an error. Any other value is an invalid request. Only Chinese candidates get a non-English gloss. */
char *msime_client_candidate_gloss_request(const uint8_t *request, size_t request_length,
                                           const uint8_t *resources, size_t resources_length);
/* 不持有会话地查询英文候选或英文释义的读音，可在宿主工作线程调用。
 * JSON 请求：{generation,items:[{text,language}],supplementary_resources?}；language 必须为 "en"。
 * text 为英文候选或一行英文释义；只读首个词组，忽略开头的 to/a/an/the，任一词未知则不返回该组。
 * 响应为 {generation,pronunciations:[{text,language,pronunciation}]}，读音为 "/…/" IPA，没有音标的项省略。
 * 优先读 resources 旁的 pronunciations/en-phonetic.db；缺失时可从 supplementary_resources 下读取同名相对路径。
 * 补充目录为可选的绝对路径，最多 4096 字节；缺省保持旧行为。两处均未安装时返回空 pronunciations，不报错。 */
char *msime_client_pronunciation_request(const uint8_t *request, size_t request_length,
                                         const uint8_t *resources, size_t resources_length);
/* Break copied Chinese candidates into words with their English, without a session, for a line a host draws under the gloss lines (never a committable gloss column).
 * JSON request: {generation,texts:[...]}, at most 64 texts. Only all-Han texts of 2 to 32 characters are considered; each is cut left to right by longest match (up to 8 characters) against character-glosses/zh-en.db (single characters first) and word-glosses/zh-en.db beside resources, and each piece shows the first phrase of its gloss: "我 I · 喜欢 to like · 你 you". A text with fewer than two glossed pieces or more than eight pieces is omitted. The response is {generation,breakdowns:[{text,breakdown}]}; with neither table installed it is empty, not an error. May run on a worker thread. */
char *msime_client_gloss_breakdown_request(const uint8_t *request, size_t request_length,
                                           const uint8_t *resources, size_t resources_length);
/* 查询固定英文词库，不创建或改变输入会话。
 * JSON 请求：{prefix,limit,correction?:false}；limit 为 1..32，响应为 {prefix,items:[...]}。
 * 缺省按原有方式补全 ASCII 字母前缀；correction:true 时接受字母与撇号，第一行始终原文，最多附加一行纠错。
 * 纠错仅处理不超过 32 字节的输入，33..128 字节原样返回；缺词库也保留原文。
 * resources 必须是资源分代目录的绝对路径；词库只读，检索结果可以缓存，可在工作线程调用。 */
char *msime_client_english_completions_request(const uint8_t *request,
                                               size_t request_length,
                                               const uint8_t *resources,
                                               size_t resources_length);
// Live per-session mode, not a persisted preference. Preserves composition and
// candidate generation; remains authoritative across preference replacement.
char *msime_client_set_chinese_punctuation(uint64_t session, bool enabled);
char *msime_client_set_character_width(uint64_t session, bool fullwidth);
char *msime_client_set_english_mode(uint64_t session, bool enabled);
/* 标出隐私会话（隐私模式、不允许学习的输入框）：不记录选词位置和上屏效率，并立即禁止、清除上屏后联想；宿主须重新读取 View，以丢弃旧候选代次。用户单独关掉学习不算隐私会话；学习本身仍由 learning 偏好决定。Value 是设下的布尔值。 */
char *msime_client_set_private_session(uint64_t session, bool enabled);
/* 组字开始前由宿主同步光标前的现成文本（最多 4096 字节 UTF-8，length 为 0 时 context 可为空指针），供重排模型与 AI 联想读取上文，空闲时不产生候选。隐私会话、安全/私密输入字段会清空已有上下文并返回 false；组字进行中返回 false 且不改状态；同步成功返回 true。 */
char *msime_client_set_surrounding_context(uint64_t session, const uint8_t *context, size_t length);
/* Android 的 next_word_prediction 偏好开启时，View.prediction=true 标记上屏后的空闲联想：editing_text、preedit 和 reading 为空；只有显式选择候选才上屏。空格、回车、数字、导航和失焦不提交联想；普通状态省略 prediction 字段，宿主缺省按 false 读取。 */
/* 中日英循环切换的状态机（`crates/client-core/src/language_cycle.rs`），一个键在 中 → 日 → 英 → 中 之间转圈。顺序来自旧版 Windows 正式版 `engine/contracts/trilingual_input.h` 的 `Next()`：中文 → 日文 → 英文 → 中文；参与循环的是「切换中英文」那一组快捷键，不是 Ctrl+Shift+J。
 * 参数：enabled 是 keybindings.trilingual_cycle；mode 是按下循环键那一刻的模式，0 中文、1 日文、2 英文；scheme 是实际在跑的方案，编号同 View.scheme；last_chinese_scheme 是要回去的中文方案（0/1/2/5/6/9），-1 表示还没记下；has_composition 是宿主会话里还有组字（已选词段或未上屏的拼音）。
 * Value 是 {mode, scheme, commit_raw}：mode 同上编号；scheme 是进入中文时要切到的方案，null 表示只换中英模式；commit_raw 为真时先原样上屏正在组的内容（已选词段保留，剩下没选的拼音串照原样出去），再换模式。value 为 null 表示这一键不循环（开关关着，这一组快捷键仍是二态切换），宿主照旧自己处理这个键。非法编号返回错误。
 * 有组字时上的屏是原始拼音串，不是高亮的候选：旧版在改名或缩写时按住循环键是为了保住已经打出来的字母，而不是替用户选一个他没要的词。 */
char *msime_client_language_cycle(bool enabled, uint8_t mode, uint8_t scheme, int8_t last_chinese_scheme,
                                  bool has_composition);
/* 报告大写锁定状态：偏好 caps_lock_ascii_punctuation 打开时，大写锁定期间没有组字的标点键（经 msime_client_character、msime_client_punctuation 或 msime_client_punctuation_with_context 送来）改走字面 ASCII 路线，和英文模式一样；punctuation_lock 为 chinese 时仍是中文标点。会话状态而非偏好，会话重建后由宿主重新报告。Value 是设下的布尔值。接上这个调用的宿主在 HostCapabilities.caps_lock_punctuation 里声明。 */
char *msime_client_set_caps_lock(uint64_t session, bool enabled);
char *msime_client_set_paired_punctuation(uint64_t session, bool enabled);
char *msime_client_set_punctuation_lock(uint64_t session, uint8_t lock);
/* 每页候选数的会话内覆盖，size 为 1–10；本平台排不下那么多时（Windows、iOS、鸿蒙最多 9）按平台上限截断，view.page_size 是实际值。组字中调用时等组字结束才生效。 */
char *msime_client_set_candidate_page_size(uint64_t session, uint8_t size);
char *msime_client_character(uint64_t session, uint8_t ascii, bool shift);
// Explicit native punctuation: finish the highlighted composition, then translate.
// Invalid non-punctuation bytes fail without modifying the session.
char *msime_client_punctuation(uint64_t session, uint8_t ascii);
/* Resolve native punctuation using the immediately preceding Unicode scalar
 * supplied by the platform editor. Zero means unavailable. Only the scalar is
 * inspected and no document text is retained. */
char *msime_client_punctuation_with_context(uint64_t session, uint8_t ascii,
                                            uint32_t preceding);

/* 无会话、无 I/O 的跨次空格策略。请求含 config（enabled/excluded_apps）、application、context（scheme/local_mode），可带 text、preceding Unicode 标量、secure、full_width。返回 {enabled,text}；先询问 enabled，再读取必要的前文。最多 65536 字节；结果按通常的 ok/value 信封返回，由 msime_client_string_free 释放。 */
char *msime_client_cross_commit_spacing(const uint8_t *request, size_t length);
/* Balance Engine nesting after the host emitted a paired closing mark. Only
 * the ASCII book-title opening '<' is accepted. */
char *msime_client_balance_paired_punctuation_after_auto_close(uint64_t session,
                                                               uint8_t opening);
// Finish the highlighted composition, then append the literal ASCII mark.
// Hosts use this for platform smart-punctuation decisions based on editor
// context; invalid non-punctuation bytes fail without modifying the session.
char *msime_client_punctuation_ascii(uint64_t session, uint8_t ascii);
/* View.scheme and commit_context.scheme: 0 quanpin, 1 shuangpin, 2 wubi, 3 japanese, 4 korean (preferences scheme "korean"). Korean is a Dubeolsik Hangul automaton: send every letter through msime_client_character with its case (Shift+Q/W/E/R/T/O/P type ㅃ ㅉ ㄸ ㄲ ㅆ ㅒ ㅖ); View.reading and View.preedit hold the composing Hangul to mark inline with the caret at its end, while editing_text holds the key letters of the open syllable and is non-empty exactly while composing. A transition may carry a commit together with a new composition (the previous syllable finished when a new one started) and a commit with handled=false (Space, Enter, a caret key, Delete or a digit ended the syllable): always insert the commit first, then let an unhandled key do its normal work in the application. Punctuation is always half-width ASCII and never converted to full width. With no Hanja list open there are no candidates: MSIME_BACKSPACE removes one jamo; MSIME_CANCEL discards the open syllable; msime_client_focus(false) commits it, while msime_client_focus(true) discards it so a syllable left open in one client never reaches the next.
 * Hanja: candidates appear only after MSIME_CONVERT_HANJA while a syllable is composing, so a host reads scheme 4 with a non-empty candidate list as "the Hanja list is open". The list holds the Hanja of the composing syllable (a syllable already committed is not converted), each candidate's annotation is its 훈음 when it has one, and its code is the key letters, which a host should not draw. MSIME_CONVERT_HANJA answers handled=false when the composition has no Hanja (a lone jamo) or nothing is composing; a host should then swallow its trigger key while composing rather than pass it on. Sending it again closes the list. While the list is open the candidate commands work as for any list (MSIME_COMMIT_CANDIDATE, digits 1-9 on the visible page, paging and MSIME_NEXT/PREVIOUS_CANDIDATE), and choosing commits the Hanja with handled=true; send MSIME_COMMIT_CANDIDATE for Return as well, since only the session knows the highlight. MSIME_CANCEL and MSIME_BACKSPACE only close the list and keep the syllable composing. A letter closes the list and composes as usual. Punctuation, MSIME_FINISH_COMPOSITION and msime_client_focus(false) close the list and commit the Hangul, never a Hanja, whatever is highlighted; msime_client_focus(true) still discards the syllable. */
/* View.scheme 和 commit_context.scheme 续：8 tibetan（藏文，偏好方案 "tibetan"），在拉丁字母键盘上按 EWTS（扩展威利转写）输入，不需要词库，所有宿主都提供，和越南文一样默认不启用。威利转写区分大小写，大写字母（T D N Sh A I U M H 等）是拼写：带大小写原样送进 msime_client_character，不要当成 Shift 命令。组字保存当前音节串的威利原文，View.preedit 和 editing_text 显示转换后的藏文，候选列表始终为空。View.spelling_symbols 空闲时为 "'/"，组字时为 "'+-./"，这些键要作为字符交给引擎，尤其 '/' 不能由宿主自己插入 ASCII 斜杠。组字时空格（字符 ' ' 或 MSIME_COMMIT_CANDIDATE）上屏藏文加音节点 U+0F0B，'/' 上屏藏文加垂符 U+0F0D，两者都是 handled=true；没有组字时 '/' 单独上屏垂符，空格和数字交回宿主。MSIME_COMMIT_RAW（回车）只上屏藏文、不加音节点，handled=true。MSIME_BACKSPACE 删掉一个原文按键；第一次 MSIME_CANCEL 把显示退回威利原文，第二次丢弃组字。数字、光标键、Delete、Tab 和 MSIME_COMMIT_READING 按显示上屏后 handled=false：先插入 commit，再让按键在应用里照常生效。其他标点先上屏藏文再跟半角 ASCII 标点，从不转成中文或全角标点。msime_client_focus(false) 上屏组字。数字保持原样，不转成藏文数字。 */
/* View.scheme and commit_context.scheme, continued: 5 cantonese (Jyutping), 6 zhuyin (Dachen bopomofo), 7 vietnamese (Telex or VNI), 9 stroke (笔画: five-stroke order 横竖撇点折 typed as the letters h s p n z, with x matching any one stroke), each the preferences scheme of the same name. Every host offers them, and Cantonese, Zhuyin and Stroke only with their language dictionaries installed; where a scheme cannot run, a preferences document naming it runs the last Chinese scheme or quanpin instead and msime_client_update_preferences says so in its diagnostic.
 * Stroke: only h s p n z start a composition; x and other letters return handled=false while idle, so a host inserts them as usual. While composing x appends a wildcard, other letters are swallowed with handled=true, and digits 1-9 pick candidates. View.reading and View.preedit hold the stroke glyphs 一丨丿丶乛＊, one per typed letter, which a host draws inline; editing_text and caret_position count the ASCII letters. MSIME_COMMIT_RAW commits the typed letters, MSIME_COMMIT_CANDIDATE the highlighted character (the letters when there is none), and focus loss does not commit.
 * MSIME_OPEN_CANDIDATE_LIST is MSIME_CONVERT_HANJA under the name that says what it does in every scheme: open the active scheme's candidate list. Korean opens and closes its Hanja list as described above; Zhuyin opens the candidate list of its composition; every other scheme answers handled=false. */
/* View.display_reading 是宿主画在行内的读音。日文罗马字（scheme 3，非本地模式、非英文）组字时末尾单个 n 还没定成ん，这里留作字母（kan 是 かn，kann 才是 かん），与微软、Google 日文输入法一致；其余方案和状态下与 View.reading 相同。View.reading 仍是 MSIME_COMMIT_READING 上屏的读音，末尾的 n 在那里已是ん。宿主不经引擎结束组字区时：光标被移走、编辑器仍报告着组字区，应把组字区换成 View.reading 再结束；确认不了组字区还在（编辑器报告没有组字区、会话停下）时只结束组字，不要把 View.reading 写进可能已被应用清空或改写的输入框，这时文档里可能留下画出的读音。只读 View.reading 的宿主行为不变。 */
/* 整句改字（全拼、双拼）：桌面宿主把不带修饰键的左右方向键发成 MSIME_CONVERSION_LEFT / MSIME_CONVERSION_RIGHT（macOS 不进改字，发 MSIME_MOVE_LEFT / MSIME_MOVE_RIGHT）；Shift+左右和 Ctrl+左右在所有方案里都发 MSIME_MOVE_LEFT_SEGMENT / MSIME_MOVE_RIGHT_SEGMENT，按音节移动拼音光标。偏好 navigation.left_right 打开时，横排候选窗里不带修饰键的左右键可能被换成移动候选（高亮不在首选，或首选上光标已在末尾时按右键），分段命令从不被换，所以高亮在哪个候选上它们都编辑拼音。首选是一个字对一个完整音节、覆盖整个组字、至少两个字的句子时，MSIME_CONVERSION_LEFT 进入改字：View.conversion 是整句，光标在 View.conversion_focus_start 个字（Unicode 标量）之前，conversion_focus_start..conversion_focus_end 是候选所替换的那一段，候选换成这一段（先是光标所在的词，再是更短、更长的）的字和词。之后左右键每次移一个字，最远到句末（focus_start 等于字数，没有候选）。选中候选（数字、MSIME_COMMIT_CANDIDATE、msime_client_select）把这段钉住，句子其余部分围绕它重新转换，光标移到这段后面，不上屏；光标在句末时 MSIME_COMMIT_CANDIDATE 上屏整句。MSIME_COMMIT_RAW、MSIME_FINISH_COMPOSITION、标点和失去焦点上屏改好的整句，不是拼音字母。MSIME_CANCEL 和 MSIME_BACKSPACE 只退出改字回到拼音（组字保留，钉住的段丢掉）；键入字母或撇号、Shift/Ctrl+左右、Home/End 先退出改字再照常处理。进不了改字时（读音不完整、单字、光标在拼音中间、其他方案、本地模式）这两条命令就是 MSIME_MOVE_LEFT / MSIME_MOVE_RIGHT。改字期间宿主把 View.phrase_prefix 加 View.conversion 画在行内代替拼音，不管预编辑样式选的是什么；改字的候选不能置顶、删除或固定位置。 */
/* 移动光标后编辑光标后的部分（偏好 caret_edit_suffix，默认关，只在三个桌面宿主上生效）：全拼、双拼的组字光标停在拼音中间（MSIME_MOVE_LEFT 等字母光标命令、分段命令）时，光标前那段原文固定为它当前的最佳转换，候选换成光标后那段的候选。此时 View.caret_edit_suffix 为 true，View.caret_edit_prefix 是固定的那段文字，首选（id.index 为 0）的 text 已经以它开头，是整句；其余候选是光标后那段开头的词。宿主在候选窗右侧显示「候选编辑：光标后」，能分段画候选的把首选开头 caret_edit_prefix 的那几个字（按 Unicode 标量数）画淡。选中一条只消耗它自己的音节：固定的那段和这一条进 View.phrase_prefix 暂存（需要 phrase_preedit），剩下的拼写接着在本模式里转换，整段完成时一起上屏。光标在开头或末尾、其他方案、本地模式和专用英文里两项都不出现，行为与关着时相同。模式里不混入英文、emoji、符号、日文、快捷短语等行，也不发云候选和 AI 查询。 */
enum MsimeCommand {
    MSIME_BACKSPACE = 0, MSIME_COMMIT_CANDIDATE = 1, MSIME_COMMIT_RAW = 2,
    MSIME_CANCEL = 3, MSIME_MOVE_LEFT = 4, MSIME_MOVE_RIGHT = 5,
    MSIME_MOVE_HOME = 6, MSIME_MOVE_END = 7, MSIME_DELETE_FORWARD = 8,
    MSIME_FINISH_COMPOSITION = 9,
    MSIME_BACKSPACE_SEGMENT = 12, MSIME_MOVE_LEFT_SEGMENT = 13, MSIME_MOVE_RIGHT_SEGMENT = 14,
    MSIME_COMMIT_RAW_WITHOUT_LEARNING = 15,
    MSIME_CONVERSION_LEFT = 17, MSIME_CONVERSION_RIGHT = 18,
    /* 宿主已经自己把文本写进了文档，再丢弃组字：对会话的作用同 MSIME_CANCEL（一次，组字没结束时照旧再发一次），区别只在生命周期里结束原因记 HostCommit、不出取消音。 */
    MSIME_CANCEL_AFTER_HOST_COMMIT = 19,
    /* 焦点即将离开或会话即将关闭，宿主替用户结束组字（macOS IMK 的 commitComposition:，宿主为词库维护关掉会话之前）：对会话的作用同 MSIME_FINISH_COMPOSITION（只有一处不同：高亮的是脚本插件在 / 指令里给的行，即来源 14 时，上屏用户打的原文，例如 /uuid，因为这时用户没在看候选窗），生命周期里上屏来源记 FocusLoss，同 msime_client_focus(false) 的上屏：不可改写、不出提交音、不画上屏特效，回复里 feedback 的 typing_effect 为 0。用户按键或点选引起的结束组字仍发 MSIME_FINISH_COMPOSITION。 */
    MSIME_FINISH_ON_FOCUS_LOSS = 20,
    MSIME_NEXT_PAGE = 100, MSIME_PREVIOUS_PAGE = 101,
    MSIME_NEXT_CANDIDATE = 102, MSIME_PREVIOUS_CANDIDATE = 103,
    MSIME_FIRST_CANDIDATE = 104, MSIME_LAST_CANDIDATE = 105,
    /* 候选窗显示时的 Home 键：高亮不在首选时同 MSIME_FIRST_CANDIDATE；已经在首选、输入串非空、没有打开的候选列表（韩文汉字、注音）且不在整句改字时同 MSIME_MOVE_HOME；整句改字时一直同 MSIME_FIRST_CANDIDATE，不退出改字。宿主只在 Home 键上发它；需要「无条件回到首选」的地方（日文转换循环候选）仍发 MSIME_FIRST_CANDIDATE。 */
    MSIME_FIRST_CANDIDATE_OR_HOME = 106
};
char *msime_client_command(uint64_t session, uint32_t command);
/* Shurufa v2 Tier 1 两阶段异步神经解码开关。 */
char *msime_client_set_neural_v2_enabled(uint64_t session, bool enabled);
/* 神经服务唤醒回调（可能从后台工作线程触发，宿主应尽快转回会话所在主线程再调用 `msime_client_poll_neural`）。 */
typedef void (*MsimeNeuralWakeupFn)(void *opaque, uint64_t session, uint64_t generation);
/* 配置并启用或关闭 Shurufa 端侧神经服务（纯 Rust 管理独立后台子进程、UDS 二进制帧、50 ms Prefill 防抖、
 * ~28 ms Provisional 防闪窗及热态 Space 条件变量有界等待）。config_json 可传 NULL（使用会话 options 默认目录）。
 * 返回 {"available": bool, "enabled": bool, "pid": number}。 */
char *msime_client_configure_neural_service(uint64_t session,
                                            const uint8_t *config_json,
                                            size_t length,
                                            MsimeNeuralWakeupFn wakeup,
                                            void *opaque);
/* 主线程收到唤醒回调后轮询：若当前代次解码已完成则按两阶段状态机融合（Provisional 期不锁槽 0，
 * Presented 期硬锁槽 0 仅升级槽 1..），或当 ~28 ms Provisional 窗到期时推进至 Presented；
 * 返回 {"ready": bool, "moved": bool, "provisional": bool, "view"?: View}。 */
char *msime_client_poll_neural(uint64_t session);
/* 用户在 ~28 ms Provisional 期内按下非字母 UI 控制键或展开候选面板时，立即结束等待窗口并转为 Presented，
 * 返回 {"flushed": bool, "provisional": false, "view"?: View}。 */
char *msime_client_flush_provisional_neural(uint64_t session);
/* 组字开始前同步光标右侧同行现成后文（最多 4096 字节 UTF-8，遇换行截断保留头部至多 16 字），供句中补字双向消歧；隐私/安全字段清空上下文并返回 false，组字中返回 false 不改状态。 */
char *msime_client_set_following_context(uint64_t session, const uint8_t *context, size_t length);
/* 取出当前代次的 Tier 1 神经解码请求（含完整保留标点/空格的 512 字上文、可选同行后文、音节 DAG 路径与候选）；不可用时返回 value=null。 */
char *msime_client_take_neural_request(uint64_t session);
/* 读取当前会话完整保留标点/空格的 512 字上文，供宿主在空闲 50 ms 后预热端侧 KV-Cache。返回 {"context": "..."}。 */
char *msime_client_neural_prefill_context(uint64_t session);
/* 将 Tier 1 神经服务返回的 NeuralDecodeResponse JSON 交给纯 Rust 引擎执行统一概率融合；
 * lock_slot_0 为 true（>80 ms 迟到）时严格保留原槽 0 不跳动，仅升级槽 1 及以后；代次过期自动丢弃。 */
char *msime_client_apply_neural_response(uint64_t session, const uint8_t *response,
                                         size_t length, bool lock_slot_0);
/* Pass the generation and global index from the displayed candidate's id. */
char *msime_client_select(uint64_t session, uint64_t generation, size_t index);
/* Select any entry copied by msime_client_all_candidates for this exact
 * generation. Normal select remains restricted to the current View page. */
char *msime_client_select_any_candidate(uint64_t session, uint64_t generation, size_t index);
char *msime_client_pin_candidate(uint64_t session, uint64_t generation, size_t index);
char *msime_client_remove_candidate(uint64_t session, uint64_t generation, size_t index);
char *msime_client_fix_candidate_position(uint64_t session, uint64_t generation, size_t index,
                                          uint8_t position);
char *msime_client_clear_candidate_position(uint64_t session, uint64_t generation, size_t index);
enum MsimeCandidateEdge { MSIME_FIRST_HAN = 0, MSIME_LAST_HAN = 1 };
/* Engine selects one Han character and clears composition on success.
 * A candidate without Han text is unhandled and keeps composition; no fallback
 * punctuation or candidate commit is manufactured. Uses the same current-page
 * identity checks as select. Invalid edge values fail before state changes.
 */
char *msime_client_select_edge(uint64_t session, uint64_t generation, size_t index, uint8_t edge);
/* On-demand {session,generation,preedit,candidates:[Candidate...]}. Unlike View,
 * candidates contains the complete cached Engine generation with global IDs. */
char *msime_client_all_candidates(uint64_t session);
/* 宿主按自己的排版显示了与固定分页不同的一组候选（macOS 横排按宽度自适应）时，报出这一代里这些候选的全局序号
 * （来自 all_candidates 的 id.index，<=256 个）。二三候选键、五笔自定义选词键和翻译查询都按看得见的这一组；
 * count 为 0 时清掉。只对 generation 这一代有效，组字推进后自动失效。
 * Value {"applied": bool}：代次过期或序号越界时为 false，原有显示页不变。 */
char *msime_client_set_display_page(uint64_t session, uint64_t generation, const size_t *indices, size_t count);
/* Return read-only English completions for a bounded ASCII prefix. */
char *msime_client_english_completions(uint64_t session, const uint8_t *prefix,
                                       size_t prefix_length, size_t limit);
/* View.local_mode 是 Engine 自己的模式，不是从预编辑前缀猜出来的；View.microsoft_shuangpin 报告 Engine 已应用的配置，不是尚未生效的新偏好；它为真表示当前是双拼且韵母或零声母编码用 `;` 作第二键，宿主要把 `;` 当字母键交给 msime_client_character。宿主结合模式、编辑文本和光标使用它。取值为 none、unicode、date_time、quick_phrase、emoji、kaomoji、super_jianpin、temporary_english、temporary_japanese、expression、command、mention、url、chaizi（拆字反查，Shift+C 进入；候选注释是字的带调读音，由 Engine 给出）、symbol（符号模式，Shift+S 进入；输入分组码 xl / sx / szq / jt 或符号名 alpha、infinity），未知取值视为不可用状态。
 * View.spelling_symbols 列出 Engine 在当前状态下当作输入的非字母键：当前模式的拼写（expression 的数字和运算符、unicode 的数字、url 的数字和网址符号）；没有组字时打开模式的键（`/` 和 `@`）；全拼、双拼、五笔组字中原文恰好是 www、http、https、ftp 时打开网址模式的键（`.` 或 `:`）。宿主要把它们当字符发送；列在里面的数字是输入，不是选候选的快捷键。转换结果的 commit_context.typing_statistics 对 expression、command、mention 模式生成的文本为 false，这些文本不计入打字统计。
 * 中文标点下 '\'' 与 '"' 出哪种引号、'[' ']' 出哪种方头括号由偏好 quote_style、bracket_style 决定（契约 shared/contracts/punctuation 的 styles 段）。View.punctuation_marks 报告引擎已应用的样式：{"quotes": "curly"|"corner", "brackets": "solid"|"hollow", "enumeration_comma_backslash": bool, "enumeration_comma_slash": bool}，corner_when_traditional 已按繁体输出开关解析成 curly 或 corner；两个顿号字段报告契约 enumerationComma 段里 '\\' 和 '/' 哪个键出顿号（至少一个为真）。宿主做成对补全、跨过后半个、空格转换、英文模式下的中文标点和画触屏键面时读它，不自己解析偏好；改了偏好要等空闲重建后它才变，和引擎出字同步。
 */
char *msime_client_view(uint64_t session);
/* 在线候选查询包含 cloud_request（GET/POST、请求头、JSON 请求体、超时、响应格式和可选缓存回包）。
 * 描述符可能含自定义服务凭据，只交给宿主传输，不写日志或上传诊断。
 * 各接收 OnlineQuery 的接口最多接受 128 KiB UTF-8 JSON；候选列表与回包沿用各自上限。
 * 云与 AI 共用这份查询，宿主在后台执行后原样交回，组字或来源变化会拒绝迟到结果。 */
#define MSIME_ONLINE_QUERY_MAX_BYTES 131072
char *msime_client_online_query(uint64_t session);
/* Build a validated AI HTTP descriptor for a copied OnlineQuery. The host
 * resolves credentials from its current preferences; the query must still
 * match the active AI configuration. */
char *msime_client_ai_request_for_query(uint64_t session,
                                        const uint8_t *query,
                                        size_t query_length);
/* 把偏好之外（例如 iOS 钥匙串）的 AI 凭据交给会话；仅覆盖当前端点的令牌，
 * 不持久化或回传，也不会随偏好切到别的端点。长度为零表示撤销。
 * 凭据确实变化时，当前组字的旧 AI 候选和旧在线查询失效；重复设置同一凭据不变。 */
char *msime_client_set_ai_credential(uint64_t session, const uint8_t *token,
                                     size_t token_length);
/* Return null or {generation,target_language,candidates:[{text}], provider:"none"|"account"|"tencent"|"niutrans"|"custom", translation_account:bool, custom_translation:{enabled,endpoint,api_key}|null, tencent_tmt:{enabled,secret_id,secret_key,region}|null, niutrans:{enabled,app_id,apikey}|null} for visible candidates.
 * provider names the service selected in preferences even when its configuration is incomplete; a transport must ask that service or none, never fall back to another. Credentials are returned only for the selected usable provider. These fields are for host-owned transport; never log the query. The existing target_language applies to both providers. translation_account is true only when the user explicitly chose the MSIME account, candidate_translations is on and no service of the user's own applies; it is the whole decision for a host's account gloss path, which must send nothing when it is false. offline_gloss_languages is present only when non-empty: the non-English target languages, in preference order, whose offline dictionary is installed beside resources or in the downloaded offline-glosses resource pack; ask msime_client_candidate_gloss_request with that target_language for each. state_root is present only when one of them comes from the pack; pass it on in that request. candidate_pronunciation is present (true) only when the user turned pronunciation on: the host then pronounces the English and Japanese gloss lines it draws, English through msime_client_pronunciation_request, as display text that is never committed. gloss_breakdown is present (true) only when candidate translation or the offline English gloss is on and English is a target language, as macOS requests breakdowns: the host then asks msime_client_gloss_breakdown_request about the sentence candidates, with resources, which the query carries whenever gloss_breakdown is present.
 * While the composition is a /fy request (command mode) the query is that request alone, whatever the gloss switches say: sentence:true, target_language "zh", one candidate {text, online_gloss:false} holding the English typed after the command, no resources, user_data or offline_gloss_languages, and null when no service of the user's own (tencent, niutrans or custom) is selected. Ask only that service, without the offline dictionary or the gloss cache, and hand the answer back through msime_client_apply_translations, which shows it as the first row and commits the translation.
 * 候选词翻译范围（candidate_translation_scope）是 "highlighted" 且候选词翻译开着时，candidates 只有当前高亮的那一个候选（高亮不在这一页时没有这一份），翻译服务和只为候选词翻译查的离线词典都只为它工作。离线释义开关（candidate_english_gloss）要查释义时，查询另带 gloss_query：一份完整的查询文档，覆盖这一页全部候选，provider 为 "none"、translation_account 为 false、各服务配置为 null，宿主用处理普通查询的同一条路径算它（不会联网），把它的每条回答标上 "gloss":true 交给 msime_client_apply_translations（同一代次，可以和主查询的回答拼成一个数组；分开交时不要交空数组）。高亮不在这一页、只剩离线释义要查时，返回的就是那份不带服务的文档本身。范围是 "all" 时没有 gloss_query，查询与以前相同。
 */
char *msime_client_translation_query(uint64_t session);
/* Google 的默认预算；cloud_request 存在时按描述符中的毫秒预算执行。 */
#define MSIME_CLOUD_CONNECT_TIMEOUT_MS 2000
#define MSIME_CLOUD_REQUEST_TIMEOUT_MS 2000
/* 兼容 GET 云候选的 URL 入口；POST 宿主直接执行 OnlineQuery.cloud_request。 */
char *msime_client_cloud_request_url(const uint8_t *query, size_t query_length);
/* 共享解析器处理宿主取得的云响应，返回 {applied,view}；空或无效响应不改变组字。
 * 查询最多 128 KiB，响应最多 256 KiB。过期、关闭或切换来源的查询不应用候选。 */
char *msime_client_apply_cloud_response(uint64_t session,
                                      const uint8_t *query, size_t query_length,
                                      const uint8_t *body, size_t body_length);
/* The #if !defined(_WIN32) blocks below mirror #[cfg(unix)] exports, so a
 * Windows host fails at compile time instead of with unresolved externals. */
#if !defined(_WIN32)
/* Linux: perform one bounded request to a user-owned Unix-socket provider.
 * Call from a worker thread with a copied query; returns null value when no
 * candidate is available. Credentials and network policy stay in that service. */
char *msime_client_online_provider_request(const uint8_t *query,
                                           size_t query_length,
                                           const uint8_t *socket_path,
                                           size_t socket_length);
#endif
/* 云剪贴板端到端加密的 sans-IO 驱动，所有平台都有。输入 {start,keystore,device_name} 开始一个操作，或 {resume,response} 交回上一步请求的响应；value 是 {request,state,keystore}、{done,keystore} 或 {error,keystore}。宿主发请求并保存 keystore，驱动不访问会话、不联网。协议见 docs/cloud-clipboard-e2e.md。请求最大 2 MiB。 */
char *msime_client_cloud_clipboard_e2e(const uint8_t *request, size_t request_length);
/* Persist {target_language,translations:[{text,translation}]} in an existing
 * absolute user-data directory. Only short changed English-target glosses are
 * saved. Candidate gloss requests may include user_data to read this overlay.
 * Maximum request size 128 KiB; path 4096 bytes. Does not access a session. */
char *msime_client_translation_gloss_save(const uint8_t *request, size_t request_length,
                                         const uint8_t *user_data, size_t user_data_length);
/* TranslationQuery accepts the optional sentence:true flag for an explicit
 * single-item sentence request (up to 512 Unicode characters); ordinary
 * candidate gloss requests retain their normal limits. */
#if !defined(_WIN32)
char *msime_client_translation_provider_request(const uint8_t *query,
                                                size_t query_length,
                                                const uint8_t *socket_path,
                                                size_t socket_length);
/* Linux standalone emoji panel adapter. The query contains search/category
 * text and a bounded result limit; the socket returns {items:[...]}. */
char *msime_client_emoji_provider_request(const uint8_t *query,
                                          size_t query_length,
                                          const uint8_t *socket_path,
                                          size_t socket_length);
#endif
/* 从已校验的 msime-others.db 读取本地表情目录；resources 为含该文件的绝对目录。
 * 无需提供商套接字，Windows 和 Unix 宿主使用同一接口。
 * offset 为非负行偏移（缺省 0），limit 为 1..255。
 * group 筛选目录子类；list_groups:true 返回 {groups:[name,...]}。
 * list_symbol_groups:true 返回 {symbol_groups:[{parent,title},...]}。
 * parent 在分页前筛选符号父类，避免不同父类下的同名子类混淆。
 * list_plugin_symbol_groups:true 搭配绝对 plugins 目录，返回
 * {plugin_symbol_groups:[{pack,pack_name,tab,title,keywords,items:[text,...]},...]}。
 * 插件组独立于内置数据库；包按名称、组按清单顺序，不与内置目录去重。
 * cursor:true 返回 next_offset 和 complete；必须按 next_offset 推进，
 * 空页可能只是坏行。缺省模式按 limit 推进，每页按文本去重。 */
char *msime_client_emoji_catalog_request(const uint8_t *query,
                                         size_t query_length,
                                         const uint8_t *resources,
                                         size_t resources_length);
/* Shared Doubao authentication policy. Input (max 32768 bytes):
 * {auth_mode,app_id,token,resource_id}; an absent or empty mode means api_key.
 * Response value: {headers:[[name,value],...]}. Contains credentials: never
 * log/persist the response; release with msime_client_string_free. */
char *msime_client_doubao_auth_headers(const uint8_t *request, size_t length);
#if !defined(_WIN32)
/* Linux voice adapter. The user-owned socket captures audio and runs ASR,
 * returning {text}; the query contains language and the active generation. */
char *msime_client_voice_provider_request(const uint8_t *query,
                                          size_t query_length,
                                          const uint8_t *socket_path,
                                          size_t socket_length);
#endif
/* Decode one Doubao v1 response frame. The value contains either {last,payload}
 * for a UTF-8 JSON response or {error_code} for a type-0xF error frame. */
char *msime_client_doubao_decode_frame(const uint8_t *frame,
                                       size_t frame_length);
/* Build caller-owned Doubao v1 start and PCM/final frames. If capacity is too
 * small, output_length receives the required size and no bytes are written. */
bool msime_client_doubao_start_frame(bool enable_itn, bool enable_punc,
                                     bool enable_ddc,
                                     const uint8_t *boosting_table_id,
                                     size_t boosting_table_id_length,
                                     uint8_t *output, size_t output_capacity,
                                     size_t *output_length);
bool msime_client_doubao_audio_frame(int32_t sequence, const uint8_t *pcm,
                                     size_t pcm_length, bool final_chunk,
                                     uint8_t *output, size_t output_capacity,
                                     size_t *output_length);
#if !defined(_WIN32)
typedef void (*msime_client_voice_update_callback)(const uint8_t *text,
                                                   size_t text_length,
                                                   bool final,
                                                   void *context);
/* Stream newline-delimited provider updates. The callback runs on the caller
 * thread and receives bounded interim/final UTF-8 text; context is untouched. */
char *msime_client_voice_provider_stream(
    const uint8_t *query, size_t query_length, const uint8_t *socket_path,
    size_t socket_length, msime_client_voice_update_callback callback,
    void *context);
/* Optional phase notifications: 0=recording, 1=recognizing, 2=polishing.
 * Both callbacks run synchronously on the caller thread and must not throw. */
typedef void (*msime_client_voice_status_callback)(uint8_t phase, void *context);
char *msime_client_voice_provider_stream_events(
    const uint8_t *query, size_t query_length, const uint8_t *socket_path,
    size_t socket_length, msime_client_voice_update_callback callback,
    msime_client_voice_status_callback status_callback, void *context);
/* Normalized microphone level in [0, 1]; never transcript text or audio.
 * Callback runs synchronously on the caller thread and must not throw.
 * All three stream calls return {"ok":true,"value":{"text":...}} on success and value null when the provider gave no result. A provider that names a missing optional dependency returns {"ok":false,"error":"voice_dependency_missing:websockets"}, "voice_dependency_missing:recorder" or "voice_dependency_missing:local_asr". */
typedef void (*msime_client_voice_level_callback)(float level, void *context);
/* Return true to stop a streaming provider read. The callback runs on the
 * provider worker at least every 100ms and must not throw. */
typedef bool (*msime_client_voice_cancellation_callback)(void *context);
char *msime_client_voice_provider_stream_feedback(
    const uint8_t *query, size_t query_length, const uint8_t *socket_path,
    size_t socket_length, msime_client_voice_update_callback callback,
    msime_client_voice_status_callback status_callback,
    msime_client_voice_level_callback level_callback, void *context);
/* Cancellable variant used by detached hosts. The legacy feedback entry point
 * remains equivalent to passing a null cancellation callback. */
char *msime_client_voice_provider_stream_feedback_cancelled(
    const uint8_t *query, size_t query_length, const uint8_t *socket_path,
    size_t socket_length, msime_client_voice_update_callback callback,
    msime_client_voice_status_callback status_callback,
    msime_client_voice_level_callback level_callback,
    msime_client_voice_cancellation_callback cancellation_callback,
    void *context);
/* Request cancellation of a provider capture session by generation. */
char *msime_client_voice_provider_cancel(const uint8_t *socket_path,
                                         size_t socket_length,
                                         uint64_t generation);
/* Ask the provider to finish capture and deliver the final stream result. */
char *msime_client_voice_provider_stop(const uint8_t *socket_path,
                                       size_t socket_length,
                                       uint64_t generation);
#endif
/* Apply a UTF-8 cloud (source=0) or AI (source=1) result for a copied query. */
char *msime_client_apply_online_candidate(uint64_t session,
                                           const uint8_t *query,
                                           size_t query_length,
                                           const uint8_t *candidate,
                                           size_t candidate_length,
                                           uint8_t source);
/* Apply a JSON array of UTF-8 strings for one source (cloud=0, AI=1).
 * Buffers are borrowed for the call; at most 16384 bytes each. */
char *msime_client_apply_online_candidates(uint64_t session,
                                          const uint8_t *query,
                                          size_t query_length,
                                          const uint8_t *candidates,
                                          size_t candidates_length,
                                          uint8_t source);
/* Remove cached and visible rows for one source (source=0 cloud, 1 AI). */
char *msime_client_clear_online_candidates(uint64_t session, uint8_t source);
/* AI 回答为空、失败，或 apply_online_candidates 返回 applied:false 时宿主会调用它（source 只认 0 和 1，其余报错）。共享内核不再放 AI 占位行（候选 source 12），AI 回答到达时第一条直接插在第一页最后一格；这个入口为已发布的宿主留着，合法的调用一律返回 {applied:false, view}，不改候选、不换代。 */
char *msime_client_resolve_online_candidates(uint64_t session,
                                             const uint8_t *query,
                                             size_t query_length,
                                             uint8_t source);
/* Resolve a shared surface route ("settings", "settings:voice", "emoji", ...)
 * so a host launches the shared shell by name. Returns the canonical route and,
 * for panel surfaces, the window label, query and geometry. */
char *msime_client_parse_surface_route(const uint8_t *value, size_t length);
/* Convert UTF-8 Simplified Chinese to Traditional Chinese with the shared
 * OpenCC s2t tables (phrase-level, so 头发 -> 頭髮 and 发展 -> 發展). Unlike
 * the JSON calls, returns the converted NUL-terminated text directly because
 * hosts call it per candidate. Returns NULL for NULL, inputs over 1 MiB,
 * invalid UTF-8 or an embedded NUL; keep the original text then. Free with
 * string_free. The first call parses the tables (a few milliseconds in
 * release builds). */
char *msime_client_simplified_to_traditional(const uint8_t *text, size_t length);
/* 同上，但按 variant 选 OpenCC 配置：0 s2t、1 s2tw（臺灣字形）、2 s2twp
 * （臺灣字形加惯用词），与偏好 traditional_chinese_variant 的 "s2t"、"s2tw"、
 * "s2twp" 一一对应。未知的 variant 与其它非法输入一样返回 NULL。臺灣配置的
 * 附加词表在第一次用到时解析。Free with string_free. */
char *msime_client_convert_to_traditional(const uint8_t *text, size_t length,
                                          uint32_t variant);
/* Display-only font aliases. Input: JSON array, at most 33 names / 32 KiB.
 * Returns the standard response with an array value. Free with string_free.
 * Resolution failure retains the corresponding original family name. */
char *msime_client_resolve_font_families(const uint8_t *value, size_t length);
/* Installed font family names, the catalog the shared app's 刷新字体列表 reads.
 * Returns the standard response with a sorted array of distinct names, each
 * non-empty, at most 128 UTF-8 bytes and free of control characters; error
 * "unsupported" on a host without a catalog, "font_catalog" or
 * "font_catalog_limit" (over 16384 families) when enumeration fails.
 * Enumerates the system's fonts: call it off the UI thread. Free with
 * string_free. */
char *msime_client_list_font_families(void);
/* Describe what the named host ("windows"/"macos"/"linux"/"android"/"ios") can
 * do, so the shared UI renders from capabilities rather than the user agent. */
char *msime_client_host_capabilities(const uint8_t *platform, size_t length);
/* Where this library reports a failure it recovers from by itself, such as a sound or music pack that does not load, an audio device that does not open, or a helpcode pack the Engine falls back from. Without a sink each report is one line on stderr prefixed "msime: "; with one, sink receives the same line without the prefix, as NUL-terminated UTF-8 valid only during the call. Each line is "<category>: <detail>": the category is a fixed English phrase saying what failed, such as "sound pack not loaded", and holds no colon, so the text before the first colon names no pack, file or cause; the detail names packs, files and error causes. Neither holds typed text, candidates or credentials; a host whose log must not keep the detail keeps only the category. Pass NULL to go back to stderr; a later call replaces the sink. A report already under way on another thread may still call the sink it read after the call that replaced or cleared it has returned, so every sink ever registered must stay callable for the rest of the process: do not unload or free its code. sink may be called on any thread, including the audio player's own and several at once, and from inside other msime_client calls: it must not block, must not call any msime_client function, and must return normally. Register it once at startup, before the first session. */
typedef void (*msime_client_diagnostic_sink)(const char *line);
void msime_client_set_diagnostic_sink(msime_client_diagnostic_sink sink);
/* Effect sounds and background music, played by this library on macOS, Windows and Linux from the session's preferences.plugins. The three calls below are for the key path: they return whether a request was queued, never block, decode or read files, and need no free. False means nothing is switched on, the session handle is unknown or on another thread, the queue is full, this platform does not play (iOS, Android, HarmonyOS), or sound failed earlier in this process, which turns it off until the process restarts with one line through msime_client_set_diagnostic_sink (stderr without one). Nothing starts - no thread, no audio device - until a call finds something switched on, so a process that never calls them (the Windows TSF DLL) pays nothing; the device is let go again after 30 s without a sound or playing music. Do not call them for keys typed into a secure (password) field.
 * key_sound: key_class 0 any other key, 1 space, 2 enter, 3 backspace; anything else queues nothing. Plays the key pack's sample for the class, or the melody pack's next note in melody mode.
 * commit_sound: call when a transition commits text. Plays the key pack's commit sample when the commit sound is on, and the next note of a melody that advances on commits.
 * music_set_active: true while the input method is active in a field that is not a secure one, false when it deactivates or a secure field gains focus; music plays only in between.
 * Achievement sounds need no call: msime_client_typing_statistics record plays one when the count passes a milestone. */
bool msime_client_key_sound(uint64_t session, uint32_t key_class);
bool msime_client_commit_sound(uint64_t session);
bool msime_client_music_set_active(uint64_t session, bool active);
/* 生命周期核心（桌面宿主选择加入；移动端宿主不调用，行为和 transition JSON 都不变）。以下调用都在会话所在线程上进行，不落盘，不分配要释放的内存；未知会话、错线程、重入调用或内部 panic 时返回 0 或 false。
 * msime_client_set_lifecycle：让会话选择加入，或换一组位；返回实际启用的位，其余位被忽略。没调用过它的会话不推导任何事件。
 *   MSIME_LIFECYCLE_FEEDBACK：按键音、上屏音、打字特效和背景音乐改由核心驱动。按键音和按键特效来自 msime_client_key；上屏音和上屏特效（typing_effect 事件 4）来自核心的上屏事件，派发类调用的回复里多一个 "feedback":{"typing_effect":N} 键，N 的位布局同 msime_client_typing_effect，没有上屏时为 0；音乐由 msime_client_set_active 仲裁所有者。本会话的 msime_client_key_sound、msime_client_commit_sound、msime_client_typing_effect（事件 0-4）和 msime_client_music_set_active 随之变为空操作（返回 false 或 0）。typing_effect 事件 5（绕过按键音的删除）不经过 msime_client_key，核心看不到，仍由宿主调用。失焦上屏（msime_client_focus(false) 和 MSIME_FINISH_ON_FOCUS_LOSS）不出声也不画。字段属性带 SECURE 时没有任何声音和特效，音乐关闭；带 FULLSCREEN 时声音静音、特效照画（同 typing_effect 的位 9）。
 *   MSIME_LIFECYCLE_COMMIT_OBSERVE_ONLY：之后的上屏只能观察、不能改写，可随时切换。TIP 已经从自己的会话把文字写进文档的路径（Windows），Server 在派发前设上、派发后清掉。设着它时脚本插件的候选变换也停下：派发里不运行 candidates，旁路调用也不重放之前的重排、隐藏和注释，列表保持引擎和内置排序的顺序。TIP 在自己的会话里选词的列表（注音等 AlwaysInlinePreedit 方案）按那个会话的顺序选，Server 一侧重排或隐藏就会让显示的顺序和数字选中的对不上，所以这类列表的每一次派发都要设着它。
 *   MSIME_LIFECYCLE_HOST_HOLDS_PIECES：宿主自己暂存部分选词（Windows），补齐整个词时才把各段拼好写进文档。这样的会话里每次部分选词都是一次上屏，commit 事件按段投递，插件读到的 commit 只是这一段；这个词的各段，包括补齐它的那一段，before_commit 都不可改写。部分选词上屏的那一段还在宿主手里、没进文档，FEEDBACK 不为它出上屏音、不画上屏特效（transition 的 feedback 为 0），整段送出的那次上屏再出；生命周期事件照常发。
 * 顺序契约。轮次：宿主每把一个物理按下交给输入法，都要在为这个键调用任何派发之前先调一次 msime_client_key，它开启新的一轮；到下一次 msime_client_key 之前的所有派发和 focus 调用都属于这一轮，同一个键可能触发多次派发（如 Windows 对同一个键再发一次 MSIME_CANCEL）。指针点选和翻页先调 msime_client_key(session, MSIME_KEY_POINTER)。带 MSIME_KEY_PASSTHROUGH 的键（Windows 交给应用、由 TIP 经 Aux 管道异步上报的键）不开新的一轮，也不承诺先于应用处理。所以"打字前"只承诺先于本轮的第一次派发。
 *   过渡期的例外：msime_client_key 还没有"开一轮但不出声"的位，所以 Linux 的 IBus 和 Fcitx5 宿主只为会出按键音的打字键调用它。松开、单独的修饰键、带 Ctrl/Alt/Super 的组合键、录音中的键和 IBus 屏幕键盘面板送来的键都不开新的一轮，它们引起的派发（如组字中按 Ctrl 组合键发出的 MSIME_FINISH_COMPOSITION 或 MSIME_CANCEL、单按 Shift 切换中英文时的 MSIME_COMMIT_RAW）算在上一轮里。第 3 阶段按轮计 fuel 的脚本运行时不会因为这类没有先调 msime_client_key 的派发熔断；补上这个位之后，宿主再为这些按下也调用它，让它们各自成一轮。按住不放产生的自动重复不出声、不画（与 macOS 宿主一样），但每次重复都会派发，所以两个 Linux 宿主为它借 msime_client_key(session, MSIME_KEY_POINTER) 开新的一轮；macOS 宿主目前不为自动重复调用 msime_client_key，自动重复引起的派发算在第一次按下那一轮里。
 * 同一次调用内的事件顺序：input（派发前）→ selection_retracted → candidate_selected → before_commit → commit → composition_end 或 composition_start → composition_update → candidates_shown 或 candidates_hidden，每次调用最多一个候选事件。focus 调用没有 input，focus_in/focus_out 排在它自己的上屏和组字结束之后；失焦上屏一律不可改写。destroy 先发 composition_end{Destroyed}，最后是会话结束。组字状态只在调用边界前后各采样一次。一次 transition 同时带着上屏和新组字时（韩文音节延续）只发 commit{continues}，不发 end 和 start。apply_online_*、resolve_online_candidates、clear_online_candidates、apply_cloud_response、rerank_settled、apply_translations 这些旁路调用，以及 set_english_mode、set_chinese_punctuation、set_punctuation_lock、update_preferences、set_ai_credential 这些可能在组字中途改动组字或候选的设置调用，只给核心发视图类事件（composition_end{Other}、candidates_shown/hidden 等）。派发、focus 和上面这些调用报错时照样推导事件：runtime 可能在报错前已经结束或改动了组字，start 和 end 始终成对。事件不带文本，也不写进任何逐键记录。
 * Windows TIP 会话的约束：TIP 在应用进程里自己的会话只给 Server 转述，不调用以下任何一个函数；字段属性和激活由 Server 的会话报告。
 * msime_client_key：key 的位 0-6 是按键类别 0-3（同 key_sound），位 7（MSIME_KEY_POINTER）是指针调用，类别被忽略，不出声不画；位 8（MSIME_KEY_REPEAT）是自动重复，画但不计连击；位 10（MSIME_KEY_PASSTHROUGH）见上。其余位忽略，类别超出 0-3 时什么都不做。返回值是 FEEDBACK 下这个键的打字特效，位布局同 msime_client_typing_effect；没开 FEEDBACK 时为 0。
 * msime_client_set_field_traits：宿主在每个键之前比较一次字段属性，变了就先调它，再调 msime_client_key。没有 MSIME_TRAIT_KNOWN 位时属性按未知处理：上屏不可改写；但 SECURE 和 FULLSCREEN 位本身仍然生效，声音、特效和音乐照样按上面的规则关闭或静音。返回现在生效的属性（不认识的位被丢掉）；会话没选择加入时返回 0。
 * msime_client_set_active：输入法在这个会话里激活（true）或停用（false）。FEEDBACK 下，true 让调用者成为背景音乐的所有者，只有所有者能开关音乐，所有者被销毁时音乐自动关闭；实际的音乐状态是"是所有者、处于激活、字段既不是 SECURE 也不是 FULLSCREEN"。会话已选择加入时返回 true。 */
#define MSIME_LIFECYCLE_FEEDBACK 0x1u
#define MSIME_LIFECYCLE_COMMIT_OBSERVE_ONLY 0x4u
#define MSIME_LIFECYCLE_HOST_HOLDS_PIECES 0x8u
#define MSIME_KEY_POINTER 0x80u
#define MSIME_KEY_REPEAT 0x100u
#define MSIME_KEY_PASSTHROUGH 0x400u
#define MSIME_TRAIT_SECURE 0x01u
#define MSIME_TRAIT_PRIVATE 0x02u
#define MSIME_TRAIT_NO_SPELLCHECK 0x04u
#define MSIME_TRAIT_FULLSCREEN 0x08u
#define MSIME_TRAIT_VOICE_ACTIVE 0x10u
#define MSIME_TRAIT_HOST_ENGLISH 0x20u
#define MSIME_TRAIT_ELEVATED 0x40u
#define MSIME_TRAIT_KNOWN 0x80000000u
uint32_t msime_client_set_lifecycle(uint64_t session, uint32_t flags);
uint32_t msime_client_key(uint64_t session, uint32_t key);
uint32_t msime_client_set_field_traits(uint64_t session, uint32_t traits);
bool msime_client_set_active(uint64_t session, bool active);
/* The typing effect of one key, for the host to draw beside the caret: the session's combo count and the resolved style: the selected effect pack's (preferences.plugins.effect_pack) when one is set, preferences.plugins.effect_style otherwise. Also for the key path: integer arithmetic on the session's own state, no lock, no allocation, no file, no free; it works on every platform, iOS, Android and HarmonyOS included. Call it once per key the host handles while effect_style is not "off", effect_pack is set or combo_counter is on, alongside key_sound, and once per commit; skip keys typed into a secure (password) field.
 * event bits 0-7: 0 any other key, 1 space, 2 enter, 3 backspace (the key_sound classes), 4 commit, 5 backspace by any other route (a delete that bypasses key_sound). 0-2 count one key; 3 and 5 end the combo; 4 counts nothing and only reports the state.
 * event bit 8 (0x100): the key is an auto-repeat of a held key; drawn, not counted. Bit 9 (0x200): sounds must stay quiet now (a full-screen foreground application on Windows); the tier-up sound is neither queued nor reported due. Other bits are ignored.
 * Return value, 0 when the resolved style is off and combo_counter is off, for an unknown session handle, a wrong thread, an event code above 5, or after a panic: bits 0-15 the combo count (saturating at 65535, always 0 while combo_counter is off); bit 16 this key moved the combo up a tier (it reached 10, 25, 50 or 100 keys); bits 17-19 the style, 0 off, 1 flash, 2 sparks, 3 power_mode; bit 20 the tier-up sound is due, set only with combo_counter and combo_tier_sound on and bit 9 clear. On macOS, Windows and Linux this library has already queued that sound (the key pack's commit sample raised 3 semitones per tier); a host that plays packs itself plays it. The combo also ends after 3000 ms without a counted key, and carries over a change of preferences. */
uint32_t msime_client_typing_effect(uint64_t session, uint32_t event);
/* The session's resolved typing effect, for drawing what msime_client_typing_effect reports. Standard response; value {pack: id|null, issue: string|null, style: "off"|"flash"|"sparks"|"power_mode", intensity: 0..100, colors: ["#RRGGBB", 0..4 entries], duration_ms: 60..1500|null, particles: 0..64|null, combo_counter: bool}. With no effect pack selected (pack null) style and intensity are preferences.plugins.effect_style and effect_intensity, colors is empty and duration_ms and particles are null: the host's own defaults for the style. With one selected, the pack's parameters replace them; a pack that cannot be loaded gives style "off" and issue says why, in Chinese for a log line. colors, duration_ms and particles are hints a host may ignore when its style has no such thing. Linux draws no effect and shows only the combo count. Reads one small manifest only when the selection or the pack changed: call it after msime_client_update_preferences and after a focus-in, not per key. Free with string_free. */
char *msime_client_typing_effect_settings(uint64_t session);
/* The validated files of one sound pack, for a host that plays packs itself (HarmonyOS). Request (<=65536 bytes): {state_root: absolute|null, sound_packs: absolute|null, pack: id}; state_root is the preferences directory holding plugins/, sound_packs the bundle's built-in pack root. Built-in ids ("default", "twinkle", "msime-typewriter", "msime-bubble", "msime-8bit", "msime-woodblock", "msime-pentatonic", "msime-canon", "msime-ode-to-joy") always resolve from sound_packs; any other id needs state_root and is refused without one. Value: {id, name, license, builtin, mode:"keys"|"sequence", sounds:{default, space, enter, backspace, commit, achievement: absolute path|null}, sequence:{sample: absolute path, semitones:[-24..24], advance:"key"|"commit"}|null, max_sample_millis, melody_idle_reset_millis}. Reads the pack from disk: not for the key path. */
char *msime_client_key_sound_pack(const uint8_t *request, size_t length);
/* The validated tracks of one music pack, for a host that streams music itself (HarmonyOS). Request (<=65536 bytes): {state_root: absolute|null, sound_packs: absolute|null, pack: id}, as for key_sound_pack. Built-in ids ("msime-music-lofi", "msime-music-ambient") resolve from sound_packs, which holds them beside the built-in sound packs; any other id needs state_root and is refused without one. Value: {id, name, license, tracks:[absolute path, in play order], max_track_seconds}; a host plays a track only when its duration is within max_track_seconds, then the next, starting over after the last. Reads the pack from disk: not for the key path. */
char *msime_client_music_pack(const uint8_t *request, size_t length);
/* 插件页的包仓库、设置页插件编辑器和 @ 名单，给桌面外壳以外的设置宿主：Windows 的原生设置应用、鸿蒙的插件页，以及 Android、iOS 设置里导入辅助码表。Request（<=8 MiB）为 {state_root: 绝对路径, sound_packs: 绝对路径|null, plugin_packs?: 绝对路径|null, script_page?: bool, user_data?: 绝对路径|null, action}；包和 mentions.json 放在 state_root/plugins 下，sound_packs 是安装包自带的内置音效包目录，plugin_packs 是桌面安装包自带的官方插件包目录（<kind>/<id>/，没给时取 sound_packs 旁边的 plugin-packs，同宿主选项的 plugin_packs）。script_page 为 true 表示宿主有脚本插件的授权页（列出、授权、排序，并写明同一用户下的其他程序能伪造授权），缺省为 false：Android、HarmonyOS 和只导入辅助码表的宿主不带它，永远看不到脚本插件。user_data 是输入法宿主选项里的 user_data 目录，只用于脚本授权的回答里的停用记录。action.operation：
 * "catalog"：value 为 {packages:[...], issues:[{kind, folder, reason}]}，即全部已安装的包、内置音效包和官方插件包（builtin 为 true，默认停用：脚本包要授权，短语表要在偏好里启用），与桌面外壳列出的相同；官方包的 id 在插件目录里有同名的旧副本时，那份副本列在 issues 里，不会被使用；不带 script_page 时去掉脚本插件（kind 为 script）的包和问题行。脚本包另有 abi、module、hooks（声明的钩子名）、permissions（申请的能力名）、memory_pages 和 content_hash（64 位小写十六进制的内容哈希，授权绑定在它上面）。
 * "import" {source: 包文件夹或 .zip 文件的绝对路径}：安装它，整包替换已安装的同 id 包；value 是装好的包。不带 script_page 时 release 构建拒绝脚本包（plugin_invalid，与移动端遇到 script 时的原因相同）；带 script_page 时接受，装好的脚本包默认不运行，要经授权。
 * "import_helpcode_table" {file_name, text}：把单个 .txt 辅助码表装成 kind 为 helpcode 的包。file_name 是用户选的文件名，只用于报错、显示名和 id；text 是码表正文，宿主必须先按 UTF-8 严格解码（解码失败就不要调用），原始字节不超过 1 MiB。码表按辅助码表包的严格规则校验（每行「字=码」，码是 1 到 2 个小写字母，同一个字只能出现一次，1 到 30000 条），不合格时是 plugin_invalid，detail 写明文件名、原因和行号。显示名取开头的 # name: 注释，没有时用去掉扩展名的文件名；id 是 custom-<小写文件名主干>，组不成合法 id 时是 custom-<12 位十六进制哈希>，同名文件再次导入时整包替换。value 是装好的包，其中 entries 是条数。宿主再把 id 写进 preferences.plugins.helpcode_pack_quanpin 或 helpcode_pack_shuangpin 来选用它。
 * "remove" {kind: "sound"|"music"|"command_table"|"effect"|"phrase_table"|"helpcode"|"wordbook"|"symbol_set"，桌面上另有 "script", id}：value 为 null；没装的包视为已删除。内置音效包和背景音乐包的 id 是 plugin_reserved；官方插件包的 id 只删插件目录里的同名旧副本，内置的那一份和它的授权都不动。
 * "load_mentions"：value 为 [{text, key, phone}]（phone 是联系人手机号，普通名单为空串），还没保存过名单时为空。保存时要把读到的 phone 原样带回去，否则它会被清掉。
 * "save_mentions" {entries:[{text, key, phone}]}：替换名单；value 为 null。
 * "authoring_load" {kind, id}：设置页插件编辑器读出用户插件目录里的一个包，value 是一份 AuthoredPack：{kind: "phrase_table"|"command_table"|"symbol_set"|"wordbook", id, name, version, license, author: string|null, description: string|null}，另加与 kind 对应的恰好一个内容字段："phrases":[{key, text}] | "commands":[{trigger, title, template}] | "groups":[{tab: "symbols"|"kaomoji", title, keywords: string|null, items:[string]}] | "words":[{word, phonetic: string|null, gloss}]（单词本读的是它的 .tsv 词表）。内置和官方包的 id、这四种以外的类型、以及没有装在 state_root/plugins 下或载入不了的包都是 plugin_not_editable，detail 说明是哪一种。
 * "authoring_save" {pack: AuthoredPack, replace: bool}：生成 plugin.toml（单词本另生成 words.tsv，一行一个「单词\t音标\t释义」，没有音标时两列）放进暂存目录，按 "import" 完全相同的规则校验并安装；value 是装好的包，与 "import" 的 value 同形。pack.id 为 null 时生成 my- 加 8 位小写十六进制随机数的 id，同类型下已被占用就再换一个；author、description、keywords 为 null 或全是空白时不写进清单。同类型、同 id 的包已经装着时，replace 为 false 是 plugin_exists，为 true 则整包替换，原包里的 .txt、.md 说明文件原样带进新包（从社区装来的包改过之后照样能保存，成为本机的副本）。内容不合规时是 plugin_invalid，detail 是清单或词表校验报告的规则（与导入同一条，例如「短语编码 DH 必须是 1 到 32 个小写字母」），另有两条只有编辑器会碰到的：单词的某一列含有制表符或换行、单词以 # 开头；内置 id 是 plugin_reserved；内容字段与 kind 不符也是 plugin_invalid。
 * 以下三项只在桌面（macOS、Windows、Linux）上有，别处是 invalid；value 都是整份授权 {revision, plugins:[{id, hash, enabled, granted:[能力名]}], health:[{id, hash, disabled: "invalid"|"init"|"failed"|"slow"}], commands:[{id, trigger, title, status: "active"|"shadowed_by_user_table"|"shadowed_by_plugin", conflict?, conflict_id?}]}，plugins 的顺序就是执行顺序，health 是输入法停用了的插件（来自 user_data/script-health.json，没给 user_data 时为空），commands 是启用、授予了 provide_commands、清单能载入的插件声明的 / 指令和状态（conflict 是冲突的触发词，conflict_id 是冲突的插件，用户指令表取 state_root 里偏好启用的指令表包），与桌面外壳的 script_grants 命令相同。
 * "script_grants"：读出授权。
 * "set_script_grant" {id, hash, enabled, granted:[能力名]}：授予、调整或撤销一个脚本插件的授权。hash 是页面展示给用户的内容哈希，包在这之后被换过时是 plugin_script_changed；granted 必须是清单申请的能力的子集，并满足依赖和 rewrite_commit 与 read_composition 互斥的规则；启用时同时启用超过 8 个是 plugin_script_limit，超出内存总账是 plugin_script_memory。撤销是 enabled 为 false、granted 为空。写下的 granted 里没有 persist_data 时（撤销了它，或者插件更新后重新授权时没有再给）同时删掉这个插件的持久存储；persist_data 与 rewrite_commit 也互斥。
 * "reorder_scripts" {ids:[id]}：ids 按给出的顺序排到最前，其余按原来的相对顺序；有重复或没有授权条目的 id 时整个拒绝。
 * 以下三项同样只在桌面上有；value 都是一个插件的状态 {id, values:{设置项 key: 值}, storage_bytes}，values 覆盖清单 [[script.settings]] 的每一项，类型同清单（bool 是 true/false，choice 是选项的 value，number 是整数，text 是字符串），存下的值不合规时是默认值；storage_bytes 是 persist_data 的存储文件占的字节数，没有时为 0。设置项的声明（title、description、type、options、min、max、max_length、default）在 "catalog" 里脚本包的 settings 字段。插件没装或清单载入不了是 plugin_script_missing。
 * "script_plugin_state" {id}：读出一个插件的设置值和存储大小。
 * "set_script_settings" {id, values:{key: 值}}：整份替换这个插件的设置值，没写的项用默认值。清单里没有的 key、类型不对或超出范围的值整个拒绝，是 plugin_script_setting_invalid（detail 说明是哪一项）；设置文件是更新的版本写的是 plugin_script_settings_format。改设置不作废授权，输入法在下一次聚焦时用新值重建插件。
 * "clear_script_data" {id}：删掉这个插件的持久存储（persist_data），输入法手上还没写盘的部分一并作废。
 * 失败时为 {ok:false, error: 失败码, detail?}：失败码与桌面外壳相同（invalid、storage、plugin_invalid、plugin_unsupported_source、plugin_archive、plugin_reserved、plugin_storage、plugin_exists、plugin_not_editable、plugin_script_missing、plugin_script_changed、plugin_script_grant_invalid、plugin_script_limit、plugin_script_memory、plugin_script_grants_format、plugin_script_setting_invalid、plugin_script_settings_format、mention_invalid、mention_format、mention_storage），detail 存在时是被拒的包或条目违反的规则，中文，给设置页显示。会读写文件，导入最多复制一个音乐包大小的数据：宿主有工作线程时在工作线程上调用。 */
char *msime_client_plugins(const uint8_t *request, size_t length);
char *msime_client_destroy(uint64_t session);
/* Write the selection counts held by every session on the calling thread and all queued personal-context learning, without ending any session. Call from the host's will-terminate hook (e.g. NSApplicationWillTerminateNotification) on the thread that owns the sessions; the C++ Engine did this from atexit. Returns null on success. */
char *msime_client_flush_all(void);
/* value must be NULL or a still-owned pointer returned by this library. */
void msime_client_string_free(char *value);

/* ===== Shurufa 零拷贝 C ABI 快照（借用会话 ViewBuffer，有效至同一会话下一次视图/派发调用） ===== */
typedef struct {
    const uint8_t *ptr;
    size_t len;
} ShurufaStringSlice;

typedef struct {
    /* 0 = curly, 1 = corner */
    uint8_t quotes;
    /* 0 = solid, 1 = hollow */
    uint8_t brackets;
    bool enumeration_comma_backslash;
    bool enumeration_comma_slash;
} ShurufaPunctuationMarks;

typedef struct {
    uint64_t session;
    uint64_t generation;
    size_t index;
    ShurufaStringSlice text;
    ShurufaStringSlice code;
    ShurufaStringSlice annotation;
    ShurufaStringSlice translation;
    uint8_t source;
    /* 0 = 未固定，1 = 置顶首位 */
    uint8_t fixed_position;
    bool has_translation;
    bool corrected;
    bool highlighted;
    bool translation_reserved;
} ShurufaCandidateItem;

typedef struct {
    uint64_t session;
    uint64_t generation;
    uint8_t scheme;
    /* 0 = Halfwidth, 1 = Fullwidth */
    uint8_t character_width;
    bool prediction;
    bool chinese_text;
    bool script_conversion;
    bool microsoft_shuangpin;
    bool dedicated_english;
    bool focused;
    bool caret_edit_suffix;
    bool provisional;
    ShurufaPunctuationMarks punctuation_marks;
    ShurufaStringSlice shuangpin_profile;
    ShurufaStringSlice local_mode;
    ShurufaStringSlice spelling_symbols;
    ShurufaStringSlice preedit;
    ShurufaStringSlice phrase_prefix;
    ShurufaStringSlice reading;
    ShurufaStringSlice display_reading;
    ShurufaStringSlice editing_text;
    size_t caret_position;
    ShurufaStringSlice conversion;
    size_t conversion_focus_start;
    size_t conversion_focus_end;
    ShurufaStringSlice caret_edit_prefix;
    size_t page;
    size_t page_size;
    size_t page_count;
    const ShurufaCandidateItem *candidates;
    size_t candidates_len;
} ShurufaViewSnapshot;

typedef struct {
    uint8_t scheme;
    bool script_conversion;
    bool typing_statistics;
    ShurufaStringSlice local_mode;
} ShurufaOutputContext;

typedef struct {
    bool handled;
    bool has_commit;
    ShurufaStringSlice commit;
    bool has_commit_context;
    ShurufaOutputContext commit_context;
    bool has_feedback;
    uint32_t typing_effect;
    ShurufaViewSnapshot view;
} ShurufaTransitionSnapshot;

/* 所有零拷贝入口在失败时返回 NULL，并在 `error_out != NULL` 时写入由 `msime_client_string_free` 释放的 UTF-8 错误串。 */
const ShurufaViewSnapshot *shurufa_session_view(uint64_t session, char **error_out);
const ShurufaTransitionSnapshot *shurufa_session_character(uint64_t session, uint8_t ascii, bool shift, char **error_out);
const ShurufaTransitionSnapshot *shurufa_session_command(uint64_t session, uint32_t command, char **error_out);
const ShurufaTransitionSnapshot *shurufa_session_punctuation(uint64_t session, uint8_t ascii, uint32_t preceding, bool has_preceding, bool ascii_literal, char **error_out);
const ShurufaTransitionSnapshot *shurufa_session_select(uint64_t session, uint64_t generation, size_t index, char **error_out);
const ShurufaTransitionSnapshot *shurufa_session_select_any(uint64_t session, uint64_t generation, size_t index, char **error_out);
const ShurufaTransitionSnapshot *shurufa_session_select_edge(uint64_t session, uint64_t generation, size_t index, uint8_t edge, char **error_out);
const ShurufaTransitionSnapshot *shurufa_session_focus(uint64_t session, bool focused, char **error_out);
const ShurufaViewSnapshot *shurufa_session_poll_neural(uint64_t session, bool *ready_out, bool *moved_out, char **error_out);
const ShurufaViewSnapshot *shurufa_session_flush_provisional_neural(uint64_t session, bool *flushed_out, char **error_out);

#ifdef __cplusplus
}
#endif
#endif
