#import "../../src/input/InputController.mm"
#import "../settings/TestPreferenceSuite.h"

#include <cassert>
#include <cstdio>
#include <vector>

// A terminal-like host: typed text goes to a pty and never comes back through the text client, so every read returns nothing and a replacement range is ignored. This is what xterm.js, iTerm2 and Terminal.app look like to an input method.
@interface TerminalLikeClient : NSObject <MSIMETextClient>
@property(nonatomic, strong) NSMutableArray<NSString *> *insertions;
@end
@implementation TerminalLikeClient
- (instancetype)init {
    if ((self = [super init])) _insertions = [NSMutableArray array];
    return self;
}
- (NSString *)bundleIdentifier { return @"com.example.terminal-like"; }
- (NSRange)selectedRange { return NSMakeRange(0, 0); }
- (NSAttributedString *)attributedSubstringFromRange:(NSRange)range { (void)range; return nil; }
- (void)insertText:(id)text replacementRange:(NSRange)range {
    (void)range;
    if ([text isKindOfClass:NSString.class]) [self.insertions addObject:text];
}
- (void)setMarkedText:(id)text selectionRange:(NSRange)selection replacementRange:(NSRange)replacement {
    (void)text; (void)selection; (void)replacement;
}
- (NSDictionary *)attributesForCharacterIndex:(NSUInteger)index lineHeightRectangle:(NSRect *)rect {
    (void)index;
    *rect = NSMakeRect(100, 100, 1, 16);
    return @{};
}
@end

// An ordinary editor that hands its text back.
@interface ReadableClient : NSObject <MSIMETextClient>
@property(nonatomic, copy) NSString *document;
@property(nonatomic) NSRange selection;
@property(nonatomic) NSUInteger replacements;
@end
@implementation ReadableClient
- (NSString *)bundleIdentifier { return @"com.example.editor"; }
- (NSRange)selectedRange { return self.selection; }
- (NSAttributedString *)attributedSubstringFromRange:(NSRange)range {
    if (range.location == NSNotFound || range.location > self.document.length ||
        range.length > self.document.length - range.location)
        return nil;
    return [[NSAttributedString alloc] initWithString:[self.document substringWithRange:range]];
}
- (void)insertText:(id)text replacementRange:(NSRange)range {
    if (![text isKindOfClass:NSString.class]) return;
    if (range.location != NSNotFound) ++self.replacements;
    NSRange target = range.location == NSNotFound ? self.selection : range;
    self.document = [self.document stringByReplacingCharactersInRange:target withString:text];
    self.selection = NSMakeRange(target.location + [text length], 0);
}
- (void)setMarkedText:(id)text selectionRange:(NSRange)selection replacementRange:(NSRange)replacement {
    (void)text; (void)selection; (void)replacement;
}
- (NSDictionary *)attributesForCharacterIndex:(NSUInteger)index lineHeightRectangle:(NSRect *)rect {
    (void)index;
    *rect = NSMakeRect(100, 100, 1, 16);
    return @{};
}
@end

// Stands in for the Engine's contextual punctuation: the smart decision is the host's, so the test only needs to see which preceding character it was given and hand back the ASCII mark it asked for.
@interface ShadowSession : NSObject
@property(nonatomic) NSUInteger contextualCalls;
@property(nonatomic) uint32_t lastPreceding;
@property(nonatomic, strong) NSMutableArray<NSString *> *surroundingContexts;
@property(nonatomic, copy) NSString *composingAfterASCII;
@end
@implementation ShadowSession
- (NSDictionary *)punctuation:(uint8_t)ascii preceding:(uint32_t)preceding error:(NSError **)error {
    (void)error;
    ++self.contextualCalls;
    self.lastPreceding = preceding;
    const unichar mark = ascii;
    return @{@"handled" : @YES, @"commit" : [NSString stringWithCharacters:&mark length:1],
             @"view" : @{@"editing_text" : @"", @"candidates" : @[]}};
}
- (BOOL)setSurroundingContext:(NSString *)context error:(NSError **)error {
    (void)error;
    if (!self.surroundingContexts) self.surroundingContexts = [NSMutableArray array];
    [self.surroundingContexts addObject:context ?: @""];
    return YES;
}
- (NSDictionary *)typeASCII:(uint8_t)character shift:(BOOL)shift error:(NSError **)error {
    (void)character; (void)shift; (void)error;
    if (self.composingAfterASCII.length) {
        return @{@"handled" : @YES,
                 @"view" : @{@"scheme" : @0, @"editing_text" : self.composingAfterASCII,
                             @"candidates" : @[@{@"text" : @"它", @"highlighted" : @YES}]}};
    }
    return nil;
}
- (NSDictionary *)hostOptions { return @{}; }
- (NSMethodSignature *)methodSignatureForSelector:(SEL)selector {
    return [super methodSignatureForSelector:selector] ?: [NSMethodSignature signatureWithObjCTypes:"@@:"];
}
- (void)forwardInvocation:(NSInvocation *)invocation {
    id result = nil;
    [invocation setReturnValue:&result];
}
@end

@interface ShadowHiddenPanel : MSIMECandidatePanel
@end
@implementation ShadowHiddenPanel
- (BOOL)isVisible { return NO; }
- (void)orderFrontRegardless {}
@end

// The posted rewrite is recorded instead of reaching the window server; `permitted` plays the Accessibility permission.
@interface ShadowController : MSIMEInputController
@property(nonatomic) BOOL permitted;
@property(nonatomic) BOOL secure;
@property(nonatomic, copy) NSString *axFallbackContext;
@property(nonatomic) NSUInteger axFallbackCalls;
@property(nonatomic, strong) NSMutableArray<NSNumber *> *posted;
@end
@implementation ShadowController
- (void)ensureAppearance {}
- (void)prepareSession {}
- (BOOL)secureEventInputActive { return self.secure; }
- (NSString *)readAXPrecedingContextForBundle:(NSString *)bundle maxUTF16Units:(NSUInteger)maxUnits {
    (void)bundle; (void)maxUnits;
    ++self.axFallbackCalls;
    return self.axFallbackContext;
}
- (BOOL)postSmartPunctuationRewrite:(unichar)replacement client:(id)client {
    (void)client;
    if (!self.permitted) return NO;
    [self.posted addObject:@(replacement)];
    return YES;
}
@end

static NSEvent *Key(unsigned short code, NSString *characters, NSEventModifierFlags flags) {
    return [NSEvent keyEventWithType:NSEventTypeKeyDown location:NSZeroPoint modifierFlags:flags timestamp:0
                        windowNumber:0 context:nil characters:characters
     charactersIgnoringModifiers:characters isARepeat:NO keyCode:code];
}

static BOOL ShadowValid(ShadowController *controller) {
    return [[controller valueForKey:@"smartPunctuationShadowValid"] boolValue];
}
static unichar Shadow(ShadowController *controller) {
    return (unichar)[[controller valueForKey:@"smartPunctuationShadow"] unsignedIntValue];
}

// A key the application typed itself: English mode hands every key to it.
static void PassThrough(ShadowController *controller, MSIMEAppearancePreferences *appearance, id client, NSEvent *event) {
    appearance.englishMode = YES;
    assert(![controller handleEvent:event client:client]);
    appearance.englishMode = NO;
}

static void TestRewriteRoute() {
    const NSTimeInterval now = NSProcessInfo.processInfo.systemUptime;
    std::vector<CGEventRef> posted;
    MSIMESmartPunctuationRewriteIO io;
    io.permitted = [] { return true; };
    io.post = [&](pid_t pid, CGEventRef event) {
        assert(pid == 4242);
        posted.push_back((CGEventRef)CFRetain(event));
    };
    io.now = [now] { return now; };
    MSIMESmartPunctuationRewrite route;
    route.pid = 4242;
    route.deadline = now + MSIMESmartPunctuationRewriteTimeout;
    route.current = [] { return true; };

    // One Delete down/up pair, then the replacement as one Unicode keystroke, every event carrying the self tag so handleEvent: lets it through to the application.
    assert(route.deliver(0x3002, io));
    assert(posted.size() == 4);
    for (size_t i = 0; i < posted.size(); ++i) {
        CGEventRef event = posted[i];
        assert(CGEventGetIntegerValueField(event, kCGEventSourceUserData) == MSIMEVoiceCommitEventTag);
        assert(CGEventGetType(event) == (i % 2 == 0 ? kCGEventKeyDown : kCGEventKeyUp));
        assert(CGEventGetFlags(event) == 0 || (CGEventGetFlags(event) & (kCGEventFlagMaskShift | kCGEventFlagMaskControl |
                                                                         kCGEventFlagMaskAlternate | kCGEventFlagMaskCommand)) == 0);
        UniChar units[4] = {};
        UniCharCount length = 0;
        CGEventKeyboardGetUnicodeString(event, 4, &length, units);
        if (i < 2) {
            assert(CGEventGetIntegerValueField(event, kCGKeyboardEventKeycode) == MSIMESmartPunctuationRewriteDeleteKey);
        } else {
            assert(length == 1 && units[0] == 0x3002);
        }
        CFRelease(event);
    }
    posted.clear();

    // No Accessibility permission: nothing is posted, and nothing asks for it.
    MSIMESmartPunctuationRewriteIO denied = io;
    denied.permitted = [] { return false; };
    assert(!route.deliver(0x3002, denied) && posted.empty());

    // The editor is no longer frontmost.
    MSIMESmartPunctuationRewrite moved = route;
    moved.current = [] { return false; };
    assert(!moved.deliver(0x3002, io) && posted.empty());

    // Past the 500 ms the attempt was given.
    MSIMESmartPunctuationRewriteIO late = io;
    late.now = [now] { return now + MSIMESmartPunctuationRewriteTimeout + 0.01; };
    assert(!route.deliver(0x3002, late) && posted.empty());

    // A client whose application is not the frontmost one gets a route that delivers nothing.
    TerminalLikeClient *elsewhere = [TerminalLikeClient new];
    MSIMESmartPunctuationRewrite captured = MSIMECaptureSmartPunctuationRewrite(elsewhere, now);
    assert(captured.pid == 0);
    assert(!captured.deliver(0x3002, io) && posted.empty());
}

int main() {
    assert(MSIMESmartPunctuationRewrite::kEventCount == 4);
    @autoreleasepool {
        [NSApplication sharedApplication];
        TestRewriteRoute();

        NSString *suite = [@"msime.smart.shadow." stringByAppendingString:NSUUID.UUID.UUIDString];
        NSUserDefaults *defaults = [[NSUserDefaults alloc] initWithSuiteName:suite];
        MSIMEAppearancePreferences *appearance = [[MSIMEAppearancePreferences alloc] initWithDefaults:defaults];
        appearance.smartPunctuation = YES;
        appearance.smartPunctuationRepeatToChinese = YES;
        appearance.smartPunctuationSpaceConvert = YES;
        appearance.pairedPunctuation = NO;
        appearance.fullWidthInput = NO;

        ShadowSession *session = [ShadowSession new];
        ShadowController *controller = [ShadowController alloc];
        controller.posted = [NSMutableArray array];
        controller.permitted = YES;
        [controller setValue:appearance forKey:@"appearance"];
        [controller setValue:session forKey:@"session"];
        [controller setValue:[[ShadowHiddenPanel alloc] init] forKey:@"panel"];
        [controller setValue:@{@"editing_text" : @"", @"candidates" : @[]} forKey:@"view"];
        auto Focus = [&](id client) {
            [controller setValue:client forKey:@"activeClient"];
            [controller resetSmartPunctuationState];
            [controller clearSmartPunctuationSpaceConversion];
            [controller clearSmartPunctuationSpaceRevert];
            [controller invalidateSmartPunctuationShadow];
            [controller.posted removeAllObjects];
        };
        NSEvent *period = Key(47, @".", 0);

        // A digit the terminal typed itself is the preceding character, so "1." commits ASCII even though the host reads back nothing.
        TerminalLikeClient *terminal = [TerminalLikeClient new];
        Focus(terminal);
        PassThrough(controller, appearance, terminal, Key(18, @"1", 0));
        assert(ShadowValid(controller) && Shadow(controller) == '1');
        assert([controller handleSmartPunctuation:period client:terminal]);
        assert(session.lastPreceding == '1');
        assert([terminal.insertions.lastObject isEqual:@"."]);
        // The commit is the new shadow.
        assert(ShadowValid(controller) && Shadow(controller) == '.');

        // The same key again within the window: the host cannot be rewritten in place, so a Delete and the Chinese mark are posted.
        const NSUInteger inserted = terminal.insertions.count;
        assert([controller handleSmartPunctuation:period client:terminal]);
        assert([controller.posted isEqual:@[ @(0x3002) ]]);
        assert(terminal.insertions.count == inserted);
        assert(ShadowValid(controller) && Shadow(controller) == 0x3002);

        // Without the posting permission nothing is rewritten; the key goes to the Engine, which types the Chinese mark after the ASCII one.
        Focus(terminal);
        controller.permitted = NO;
        PassThrough(controller, appearance, terminal, Key(18, @"1", 0));
        assert([controller handleSmartPunctuation:period client:terminal]);
        const NSUInteger beforeDenied = terminal.insertions.count;
        assert(![controller handleSmartPunctuation:period client:terminal]);
        assert(controller.posted.count == 0 && terminal.insertions.count == beforeDenied);
        controller.permitted = YES;

        // Caret and editing keys, and chords, leave the caret where the shadow cannot follow: "." after them is decided by the document, which here says nothing, so the Engine gets the key.
        for (NSEvent *breaker in @[ Key(123, @"", 0), Key(51, @"\x7F", 0), Key(53, @"\x1B", 0), Key(117, @"", 0),
                                    Key(36, @"\r", 0), Key(48, @"\t", 0), Key(115, @"", 0), Key(121, @"", 0),
                                    Key(9, @"v", NSEventModifierFlagCommand), Key(0, @"a", NSEventModifierFlagControl),
                                    Key(18, @"¡", NSEventModifierFlagOption) ]) {
            Focus(terminal);
            PassThrough(controller, appearance, terminal, Key(18, @"1", 0));
            assert(ShadowValid(controller));
            PassThrough(controller, appearance, terminal, breaker);
            assert(!ShadowValid(controller));
            const NSUInteger calls = session.contextualCalls;
            assert(![controller handleSmartPunctuation:period client:terminal]);
            assert(session.contextualCalls == calls);
        }

        // Shift is how many printable characters are typed, and it keeps the shadow.
        Focus(terminal);
        PassThrough(controller, appearance, terminal, Key(0, @"A", NSEventModifierFlagShift));
        assert(ShadowValid(controller) && Shadow(controller) == 'A');

        // An eaten key that committed nothing fed a composition; the shadow gives way to the document. One that committed keeps what the commit recorded.
        [controller noteKeyForSmartPunctuationShadow:Key(45, @"n", 0) eaten:YES];
        assert(!ShadowValid(controller));
        [controller apply:@{@"handled" : @YES, @"commit" : @"v1", @"view" : @{@"editing_text" : @"", @"candidates" : @[]}}];
        [controller noteKeyForSmartPunctuationShadow:Key(36, @"\r", 0) eaten:YES];
        assert(ShadowValid(controller) && Shadow(controller) == '1');

        // A commit's last character is the shadow, so "v1" committed and then "." commits ASCII.
        Focus(terminal);
        [controller apply:@{@"handled" : @YES, @"commit" : @"v1", @"view" : @{@"editing_text" : @"", @"candidates" : @[]}}];
        assert(ShadowValid(controller) && Shadow(controller) == '1');
        assert([controller handleSmartPunctuation:period client:terminal]);
        assert(session.lastPreceding == '1');

        // Events this host posted itself come back tagged; they are left to the application and do not move the shadow.
        CGEventRef tagged = CGEventCreateKeyboardEvent(nullptr, 0, true);
        const UniChar x = 'x';
        CGEventKeyboardSetUnicodeString(tagged, 1, &x);
        CGEventSetIntegerValueField(tagged, kCGEventSourceUserData, MSIMEVoiceCommitEventTag);
        NSEvent *selfPosted = [NSEvent eventWithCGEvent:tagged];
        CFRelease(tagged);
        const unichar before = Shadow(controller);
        assert(![controller handleEvent:selfPosted client:terminal]);
        assert(ShadowValid(controller) && Shadow(controller) == before);

        // Space after a Chinese mark in a terminal: the committed mark is the shadow, and the ASCII form is posted over it. The same key then posts the Chinese mark back.
        Focus(terminal);
        [controller apply:@{@"handled" : @YES, @"commit" : @"你好，", @"view" : @{@"editing_text" : @"", @"candidates" : @[]}}];
        [controller noteCommittedChinesePunctuation:@{@"commit" : @"你好，"} key:',' client:terminal];
        assert([controller convertSmartPunctuationSpace:Key(49, @" ", 0) client:terminal]);
        assert([controller.posted isEqual:@[ @',' ]]);
        assert(Shadow(controller) == ',');
        assert([controller revertSmartPunctuationSpace:Key(43, @",", 0) client:terminal]);
        assert([controller.posted isEqual:(@[ @',', @(0xFF0C) ])]);
        assert(Shadow(controller) == 0xFF0C);

        // Without the permission the space is handed back and nothing is posted.
        Focus(terminal);
        controller.permitted = NO;
        [controller apply:@{@"handled" : @YES, @"commit" : @"，", @"view" : @{@"editing_text" : @"", @"candidates" : @[]}}];
        [controller noteCommittedChinesePunctuation:@{@"commit" : @"，"} key:',' client:terminal];
        assert(![controller convertSmartPunctuationSpace:Key(49, @" ", 0) client:terminal]);
        assert(controller.posted.count == 0);
        controller.permitted = YES;

        // With no shadow there is no evidence of what is on screen, so nothing is posted.
        Focus(terminal);
        [controller noteCommittedChinesePunctuation:@{@"commit" : @"，"} key:',' client:terminal];
        assert(![controller convertSmartPunctuationSpace:Key(49, @" ", 0) client:terminal]);
        assert(controller.posted.count == 0);

        // A host that reads its text back keeps the in-place rewrite and never posts.
        ReadableClient *editor = [ReadableClient new];
        editor.document = @"1";
        editor.selection = NSMakeRange(1, 0);
        Focus(editor);
        assert([controller handleSmartPunctuation:period client:editor]);
        assert([editor.document isEqual:@"1."]);
        assert([controller handleSmartPunctuation:period client:editor]);
        assert([editor.document isEqual:@"1。"] && editor.replacements == 1);
        assert(controller.posted.count == 0);

        [controller apply:@{@"handled" : @YES, @"commit" : @"，", @"view" : @{@"editing_text" : @"", @"candidates" : @[]}}];
        [controller noteCommittedChinesePunctuation:@{@"commit" : @"，"} key:',' client:editor];
        assert([controller convertSmartPunctuationSpace:Key(49, @" ", 0) client:editor]);
        assert([editor.document isEqual:@"1。,"] && controller.posted.count == 0);

        // A readable document that disagrees with what was armed is left alone rather than rewritten by posting.
        ReadableClient *movedEditor = [ReadableClient new];
        movedEditor.document = @"abc";
        movedEditor.selection = NSMakeRange(3, 0);
        Focus(movedEditor);
        [controller noteSmartPunctuationShadow:0xFF0C];
        [controller noteCommittedChinesePunctuation:@{@"commit" : @"，"} key:',' client:movedEditor];
        assert(![controller convertSmartPunctuationSpace:Key(49, @" ", 0) client:movedEditor]);
        assert([movedEditor.document isEqual:@"abc"] && controller.posted.count == 0);

        // A readable editor whose selection the shadow never saw (a mouse drag does not reach handleEvent:): the preceding read comes back empty because of the selection, not because the host hides its text, so a posted Delete would erase the selection. Nothing is posted and the space falls through.
        ReadableClient *selecting = [ReadableClient new];
        selecting.document = @"";
        selecting.selection = NSMakeRange(0, 0);
        Focus(selecting);
        [controller apply:@{@"handled" : @YES, @"commit" : @"你好，", @"view" : @{@"editing_text" : @"", @"candidates" : @[]}}];
        [controller noteCommittedChinesePunctuation:@{@"commit" : @"你好，"} key:',' client:selecting];
        assert([selecting.document isEqual:@"你好，"] && ShadowValid(controller) && Shadow(controller) == 0xFF0C);
        selecting.selection = NSMakeRange(0, 2);
        assert(![controller convertSmartPunctuationSpace:Key(49, @" ", 0) client:selecting]);
        assert([selecting.document isEqual:@"你好，"] && controller.posted.count == 0);

        // The same editor with the caret clicked to the start of the document: the first character reads back, so the host is readable and nothing is posted there either.
        Focus(selecting);
        selecting.selection = NSMakeRange(selecting.document.length, 0);
        [controller apply:@{@"handled" : @YES, @"commit" : @"，", @"view" : @{@"editing_text" : @"", @"candidates" : @[]}}];
        [controller noteCommittedChinesePunctuation:@{@"commit" : @"，"} key:',' client:selecting];
        NSString *committed = selecting.document;
        selecting.selection = NSMakeRange(0, 0);
        assert(![controller convertSmartPunctuationSpace:Key(49, @" ", 0) client:selecting]);
        assert([selecting.document isEqual:committed] && controller.posted.count == 0);

        // Repeat-to-Chinese after the caret moved to the start: no posted rewrite, and the key goes on to the ordinary path.
        ReadableClient *repeating = [ReadableClient new];
        repeating.document = @"1";
        repeating.selection = NSMakeRange(1, 0);
        Focus(repeating);
        assert([controller handleSmartPunctuation:period client:repeating]);
        assert([repeating.document isEqual:@"1."]);
        repeating.selection = NSMakeRange(0, 0);
        [controller handleSmartPunctuation:period client:repeating];
        assert(controller.posted.count == 0 && repeating.replacements == 0);
        assert([repeating.document hasSuffix:@"1."]);

        // Switching clients, or losing focus, forgets the shadow.
        Focus(terminal);
        PassThrough(controller, appearance, terminal, Key(18, @"1", 0));
        assert(ShadowValid(controller));
        [controller handleEvent:Key(18, @"1", 0) client:nil];
        assert(!ShadowValid(controller));

        // 敲拼音开始组字时，仅在首枚字母键同步一次光标前现成文本；组字中后续字母、空闲态、安全输入和终端均不读取。
        ReadableClient *contextEditor = [ReadableClient new];
        contextEditor.document = @"这是一只小猫，";
        contextEditor.selection = NSMakeRange(contextEditor.document.length, 0);
        Focus(contextEditor);
        [controller setValue:@{@"scheme" : @0, @"editing_text" : @"", @"candidates" : @[]} forKey:@"view"];
        session.surroundingContexts = [NSMutableArray array];
        session.composingAfterASCII = @"t";
        assert([controller handleEvent:Key(17, @"t", 0) client:contextEditor]);
        assert(session.surroundingContexts.count == 1 && [session.surroundingContexts[0] isEqual:@"这是一只小猫，"]);
        assert(controller.axFallbackCalls == 0);
        // 同一轮组字的第 2 枚字母不再回读文档。
        session.composingAfterASCII = @"ta";
        assert([controller handleEvent:Key(0, @"a", 0) client:contextEditor]);
        assert(session.surroundingContexts.count == 1);

        // 光标挪到非空文档起点（0 位）或删空已知可读文档时，把上下文同步为空串以切断旧前文。
        [controller setValue:@{@"scheme" : @0, @"editing_text" : @"", @"candidates" : @[]} forKey:@"view"];
        contextEditor.selection = NSMakeRange(0, 0);
        session.composingAfterASCII = @"t";
        assert([controller handleEvent:Key(17, @"t", 0) client:contextEditor]);
        assert(session.surroundingContexts.count == 2 && [session.surroundingContexts[1] isEqual:@""]);

        [controller setValue:@{@"scheme" : @0, @"editing_text" : @"", @"candidates" : @[]} forKey:@"view"];
        contextEditor.document = @"";
        contextEditor.selection = NSMakeRange(0, 0);
        assert([controller handleEvent:Key(17, @"t", 0) client:contextEditor]);
        assert(session.surroundingContexts.count == 3 && [session.surroundingContexts[2] isEqual:@""]);

        // 安全输入（密码框）下不读取、不同步现成文本。
        [controller setValue:@{@"scheme" : @0, @"editing_text" : @"", @"candidates" : @[]} forKey:@"view"];
        contextEditor.document = @"秘密文本";
        contextEditor.selection = NSMakeRange(contextEditor.document.length, 0);
        controller.secure = YES;
        assert([controller handleEvent:Key(17, @"t", 0) client:contextEditor]);
        assert(session.surroundingContexts.count == 3);
        controller.secure = NO;

        // 不回传文档的 Electron/Chromium 类客户端触发 AX 兜底；AX 也为空时保留会话自身的上屏历史而不覆盖。
        TerminalLikeClient *electronLike = [TerminalLikeClient new];
        Focus(electronLike);
        [controller setValue:@{@"scheme" : @0, @"editing_text" : @"", @"candidates" : @[]} forKey:@"view"];
        controller.axFallbackCalls = 0;
        controller.axFallbackContext = @"来自AX的前文";
        assert([controller handleEvent:Key(17, @"t", 0) client:electronLike]);
        assert(controller.axFallbackCalls == 1 && session.surroundingContexts.count == 4 &&
               [session.surroundingContexts[3] isEqual:@"来自AX的前文"]);
        [controller setValue:@{@"scheme" : @0, @"editing_text" : @"", @"candidates" : @[]} forKey:@"view"];
        controller.axFallbackContext = nil;
        assert([controller handleEvent:Key(17, @"t", 0) client:electronLike]);
        assert(controller.axFallbackCalls == 2 && session.surroundingContexts.count == 4);

        session.composingAfterASCII = nil;
        MSIMERemoveTestPreferenceSuite(defaults, suite);
    }
    std::puts("macOS smart punctuation shadow passed.");
    return 0;
}
