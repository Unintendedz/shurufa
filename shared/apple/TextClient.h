#import <Foundation/Foundation.h>
#include "msime_client.h"

@protocol MSIMETextClient <NSObject>
- (void)insertText:(id)text replacementRange:(NSRange)range;
- (void)setMarkedText:(id)text selectionRange:(NSRange)selection replacementRange:(NSRange)replacement;
@optional
// NSTextInputClient-compatible context queries. Hosts that cannot expose
// document context may omit these; callers must treat the result as unknown.
- (NSRange)selectedRange;
- (NSInteger)length;
- (NSAttributedString *)attributedSubstringFromRange:(NSRange)range;
@end

typedef NS_ENUM(NSInteger, MSIMEInlinePreeditStyle) {
    MSIMEInlinePreeditStyleRaw,
    MSIMEInlinePreeditStylePinyin,
    MSIMEInlinePreeditStyleEmpty,
};

void MSIMEApplyTransition(NSDictionary *transition, id<MSIMETextClient> client);
/// Applies a transition using the shared inline preedit display preference.
/// The legacy entry point above remains the pinyin-display default for callers
/// that do not consume shared preferences yet.
void MSIMEApplyTransitionWithPreeditStyle(NSDictionary *transition, id<MSIMETextClient> client,
                                          MSIMEInlinePreeditStyle style);
/// The same, carrying a closing mark the host owes the document.
///
/// Paired punctuation puts the caret between the two marks, and IMK has no way to move a client's
/// insertion point. The opening mark is committed as usual and the closing one rides in the marked
/// text after the caret until the composition ends, so what the user sees is `（|）` and then
/// `（你好|）`. Passing nil is the ordinary case and behaves exactly as the call above.
void MSIMEApplyTransitionWithPendingClosing(NSDictionary *transition, id<MSIMETextClient> client,
                                            MSIMEInlinePreeditStyle style, NSString *closing);
/// The same, for a host that knows whether the client still holds marked text this host wrote.
///
/// Every call into an IMK client is a synchronous round trip into another process, and a focus change makes one while the client is itself blocked waiting for the input method to activate: the two stall each other until IMK's XPC timeout, which is the 1-3 s hitch Chrome shows on every tab or window switch. Clearing marked text that was never set is the one write that changes nothing, so with nothing to commit, nothing to mark and `*clientHasMarkedText` NO the client is not called at all. A commit still sends the clear after it, exactly as above. `*clientHasMarkedText` is updated to what the client holds afterwards; NULL means unknown and always sends.
void MSIMEApplyTransitionTrackingMarkedText(NSDictionary *transition, id<MSIMETextClient> client,
                                            MSIMEInlinePreeditStyle style, NSString *closing,
                                            BOOL *clientHasMarkedText);
/// 客户端同步写入后先运行 observer，再判定是否仍可写旧预编辑；observer 接收含待闭合符的实际上屏文本。
void MSIMEApplyTransitionWithCommitObserver(NSDictionary *transition, id<MSIMETextClient> client,
                                            MSIMEInlinePreeditStyle style, NSString *closing,
                                            BOOL *clientHasMarkedText, BOOL (^isCurrent)(void),
                                            void (^didCommit)(NSString *text));
/// 同步客户端调用可能处理较新的转换；宿主判定旧转换过期后，不再写它的预编辑。
void MSIMEApplyTransitionWithWriteGuard(NSDictionary *transition, id<MSIMETextClient> client,
                                       MSIMEInlinePreeditStyle style, NSString *closing,
                                       BOOL *clientHasMarkedText, BOOL (^isCurrent)(void));
/// 直接按零拷贝 C 快照同步写入上屏与行内预编辑（热路径不再经过 `NSDictionary` 装箱）。
void MSIMEApplyTransitionSnapshotWithCommitObserver(const ShurufaTransitionSnapshot *transition,
                                                    NSString *overrideCommit,
                                                    id<MSIMETextClient> client,
                                                    MSIMEInlinePreeditStyle style, NSString *closing,
                                                    BOOL *clientHasMarkedText, BOOL (^isCurrent)(void),
                                                    void (^didCommit)(NSString *text));
// UTF-16 display offset shared by marked text and the candidate preedit row.
NSUInteger MSIMEPreeditCaretPosition(NSString *editing, NSString *preedit, id position);

// Returns the single UTF-16 character immediately following the selection,
// or nil when the host cannot safely expose document context.
NSString * MSIMETextClientFollowingCharacter(id<MSIMETextClient> client);
/// Returns the Unicode scalar immediately preceding the selection, or zero when
/// the host cannot safely expose document context.
uint32_t MSIMETextClientPrecedingUnicodeScalar(id<MSIMETextClient> client);

/// 读取选区或折叠光标前至多 `maxUTF16Units` 个 UTF-16 码元的现成文本；客户端无法暴露文档时返回 `nil`。
/// `knownReadable` 表示该客户端此前已成功返回过文档片段，此时光标位于 0 返回空串 `@""`；`provedReadable` 输出本次是否确认可读。
NSString *MSIMETextClientPrecedingContext(id<MSIMETextClient> client, NSUInteger maxUTF16Units,
                                          BOOL knownReadable, BOOL *provedReadable);

/// 读取选区或折叠光标后至多 `maxUTF16Units` 个 UTF-16 码元的现成后文（供句中补字双向消歧）；不可读或越界时安全返回 `nil`。
NSString *MSIMETextClientFollowingContext(id<MSIMETextClient> client, NSUInteger maxUTF16Units);

/// 校验刚上屏文本仍紧邻折叠光标，返回其文档范围与左侧标量；未知、过长或过期时返回 NSNotFound。
NSRange MSIMETextClientCommittedRange(id<MSIMETextClient> client, NSString *text,
                                      uint32_t *preceding, BOOL (^isCurrent)(void));
