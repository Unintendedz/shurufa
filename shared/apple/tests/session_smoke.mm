#import "MSIMEClientSession.h"
#import "TextClient.h"
#include "msime_client.h"
#include <cassert>
#include <initializer_list>

@interface SmokeClient : NSObject <MSIMETextClient>
@property(nonatomic, copy) NSString *committed;
@end

@implementation SmokeClient
- (void)insertText:(id)text replacementRange:(NSRange)range {
    (void)range;
    if ([text isKindOfClass:NSAttributedString.class]) self.committed = ((NSAttributedString *)text).string;
    else if ([text isKindOfClass:NSString.class]) self.committed = text;
}
- (void)setMarkedText:(id)text selectionRange:(NSRange)selection replacementRange:(NSRange)replacement {
    (void)text; (void)selection; (void)replacement;
}
@end

static NSDictionary *reload(MSIMEClientSession *session, NSString *directory, BOOL expectError) {
    __block BOOL done = NO;
    __block NSDictionary *loaded = nil;
    [session reloadPreferencesDirectory:directory completion:^(NSDictionary *result, NSError *error) {
        assert([NSThread isMainThread]);
        assert((error != nil) == expectError);
        loaded = result;
        done = YES;
    }];
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:5];
    while (!done && deadline.timeIntervalSinceNow > 0) {
        [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
    }
    assert(done);
    return loaded;
}

int main() {
    @autoreleasepool {
        MSIMEApplyTransition((NSDictionary *)@[], nil);
        NSString *root = [NSTemporaryDirectory() stringByAppendingPathComponent:NSUUID.UUID.UUIDString];
        NSMutableDictionary *options = [@{@"api_version": @1, @"preferences": @{@"scheme": @"quanpin", @"default_ime_mode": @"chinese", @"candidate_page_size": @5, @"learning": @NO, @"chinese_punctuation": @YES}} mutableCopy];
        for (NSString *name in @[@"resources", @"user_data", @"cache", @"dictionaries"]) {
            NSString *path = [root stringByAppendingPathComponent:name];
            BOOL created = [[NSFileManager defaultManager] createDirectoryAtPath:path withIntermediateDirectories:YES attributes:nil error:nil];
            assert(created);
            options[name] = path;
        }
        NSError *error = nil;
        assert([MSIMEClientSession emojiCatalogRequest:(NSDictionary *)@[]][@"error"]);
        MSIMEClientSession *session = [[MSIMEClientSession alloc] initWithOptions:options error:&error];
        assert(session && !error);
        // 云描述符与较长 AI 配置共用查询；Apple 桥接不能在 Rust 之前按旧 16 KiB 拒绝。
        NSString *prompt = [@"s" stringByPaddingToLength:8000 withString:@"s" startingAtIndex:0];
        NSDictionary *largeAI = @{ @"enabled": @YES, @"provider": @"custom", @"model": @"synthetic",
            @"endpoint": @"https://ai.example.invalid/v1/chat/completions", @"prompt_custom_1": prompt,
            @"prompt_custom_2": prompt, @"prompt_custom_3": prompt };
        NSDictionary *largeQuery = @{ @"scheme": @0, @"generation": @1, @"identity": @"synthetic", @"query_text": @"nihao",
            @"cache_key": @"synthetic", @"pinyin_segments": @[@"ni", @"hao"], @"cloud_eligible": @YES,
            @"cloud_candidates": @YES, @"ai_eligible": @NO, @"session_id": @1, @"ai_assistant": largeAI };
        error = nil;
        assert([MSIMEClientSession cloudRequestURLForQuery:largeQuery error:&error] && !error);
        NSString *largeRoot = [root stringByAppendingPathComponent:@"large-cloud-preferences"];
        NSMutableDictionary *largePreferences = [options[@"preferences"] mutableCopy];
        largePreferences[@"ai_assistant"] = largeAI;
        NSDictionary *largeSaved = [MSIMEClientSession savePreferencesInDirectory:largeRoot expectedRevision:0
            snapshot:@{ @"format_version": @1, @"revision": @0, @"preferences": largePreferences } error:&error];
        assert(largeSaved && !error);
        assert([session.hostOptions[@"api_version"] isEqual:@1]);
        error = nil;
        NSDictionary *initialPreferences = [MSIMEClientSession loadPreferencesInDirectory:root error:&error];
        assert(initialPreferences && !error && [initialPreferences[@"revision"] isEqual:@0]);
        error = nil;
        assert(![MSIMEClientSession prepareHostWithResourcesDirectory:@"relative/resources" stateRoot:root error:&error] && error);
        error = nil;
        assert(![MSIMEClientSession prepareHostWithResourcesDirectory:@"" stateRoot:root error:&error] && error);
        error = nil;
        NSDictionary *saved = [MSIMEClientSession savePreferencesInDirectory:root expectedRevision:0 snapshot:@{@"format_version": @1, @"revision": @0, @"preferences": options[@"preferences"]} error:&error];
        assert(saved && !error && [saved[@"revision"] isEqual:@1]);
        error = nil;
        NSDictionary *stale = @{ @"format_version": @1, @"revision": @0, @"preferences": options[@"preferences"] };
        assert(![MSIMEClientSession savePreferencesInDirectory:root expectedRevision:0 snapshot:stale error:&error] && error);
        error = nil;
        NSDictionary *unsupported = @{ @"format_version": @2, @"revision": @1, @"preferences": options[@"preferences"] };
        assert(![MSIMEClientSession savePreferencesInDirectory:root expectedRevision:1 snapshot:unsupported error:&error] && error);
        NSDictionary *invalidDictionary = [MSIMEClientSession dictionaryRequest:@{@"options": options, @"action": @{@"operation": @"unknown"}} error:&error];
        assert(!invalidDictionary && error);
        error = nil;
        assert(![session voiceProviderCancelSocket:(NSString *)@YES generation:1 error:&error] && error);
        error = nil;
        assert(![session voiceProviderStopSocket:(NSString *)@YES generation:1 error:&error] && error);
        error = nil;
        assert(![session applyVoiceText:(NSString *)@YES generation:1 error:&error] && error);
        error = nil;
        assert(![MSIMEClientSession prepareHostWithResourcesDirectory:(NSString *)@YES stateRoot:root error:&error] && error);
        error = nil;
        assert(![MSIMEClientSession loadPreferencesInDirectory:(NSString *)@YES error:&error] && error);
        error = nil;
        NSMutableDictionary *oversized = [@{} mutableCopy];
        oversized[@"padding"] = [@"x" stringByPaddingToLength:70000 withString:@"x" startingAtIndex:0];
        assert(![MSIMEClientSession dictionaryRequest:oversized error:&error] && error);
        assert([session setFocused:YES error:&error]);
        assert([session typeASCII:'U' shift:YES error:&error]);
        for (uint8_t key : {'4', 'e', '2', 'd'}) assert([session typeASCII:key shift:NO error:&error]);
        NSDictionary *snapshot = @{@"format_version": @1, @"revision": @1, @"preferences": @{@"scheme": @"quanpin", @"candidate_page_size": @2, @"learning": @NO, @"chinese_punctuation": @NO}};
        NSDictionary *queued = [session updatePreferencesSnapshot:snapshot error:&error];
        assert([queued[@"deferred"] isEqual:@YES]);
        NSDictionary *result = [session command:MSIME_COMMIT_CANDIDATE error:&error];
        assert([result[@"commit"] isEqual:@"中"]);
        assert([[session updatePreferencesSnapshot:snapshot error:&error][@"deferred"] isEqual:@NO]);
        assert([[session typeASCII:',' shift:NO error:&error][@"handled"] isEqual:@NO]);
        NSString *preferencesPath = [root stringByAppendingPathComponent:@"config.json"];
        NSDictionary *on = @{@"format_version": @1, @"revision": @2, @"preferences": options[@"preferences"]};
        assert([[NSJSONSerialization dataWithJSONObject:on options:0 error:nil] writeToFile:preferencesPath atomically:YES]);
        assert([session typeASCII:'U' shift:YES error:&error]);
        for (uint8_t key : {'4', 'e', '2', 'd'}) assert([session typeASCII:key shift:NO error:&error]);
        assert([reload(session, root, NO)[@"deferred"] isEqual:@YES]);
        assert([[session command:MSIME_COMMIT_CANDIDATE error:&error][@"commit"] isEqual:@"中"]);
        assert([[session typeASCII:',' shift:NO error:&error][@"commit"] isEqual:@"，"]);
        assert([[@"broken" dataUsingEncoding:NSUTF8StringEncoding] writeToFile:preferencesPath atomically:YES]);
        assert(!reload(session, root, YES));
        assert([[session typeASCII:',' shift:NO error:&error][@"commit"] isEqual:@"，"]);
        __block BOOL rejected = NO;
        assert([session setChinesePunctuationEnabled:NO error:&error]);
        assert([[session typeASCII:',' shift:NO error:&error][@"handled"] isEqual:@NO]);
        assert([session setChinesePunctuationEnabled:YES error:&error]);
        assert([[session typeASCII:',' shift:NO error:&error][@"commit"] isEqual:@"，"]);
        // With paired completion off (the macOS host also sends this for an excluded app) no host supplies the closing half, so the Engine alone alternates quotes and nests book titles, as the reference does regardless of the setting (crates/engine/src/punctuation.rs).
        assert([session setPairedPunctuationEnabled:NO error:&error]);
        const struct { uint8_t key; NSString *mark; } unpaired[] = {
            {'"', @"“"}, {'"', @"”"}, {'"', @"“"}, {'"', @"”"}, {'\'', @"‘"}, {'\'', @"’"},
            {'<', @"《"}, {'<', @"〈"}, {'>', @"〉"}, {'>', @"》"}, {'>', @"》"}, {'<', @"《"}, {'>', @"》"},
        };
        for (const auto &step : unpaired) assert([[session punctuation:step.key error:&error][@"commit"] isEqual:step.mark]);
        assert([session setPairedPunctuationEnabled:YES error:&error]);
        error = nil;
        assert([[session resetCacheWithError:&error][@"handled"] isEqual:@YES] && !error);
        dispatch_semaphore_t done = dispatch_semaphore_create(0);
        dispatch_async(dispatch_get_global_queue(QOS_CLASS_DEFAULT, 0), ^{
            NSError *threadError = nil;
            rejected = ![session viewWithError:&threadError] && threadError != nil;
            rejected = rejected && ![session setChinesePunctuationEnabled:NO error:&threadError] && threadError != nil;
            dispatch_semaphore_signal(done);
        });
        assert(dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, 5 * NSEC_PER_SEC)) == 0);
        assert(rejected);
        // Nothing under preferences.plugins is on, so the sound calls queue nothing and start no player; a closed session answers the same without reaching the library.
        assert(![session keySound:0] && ![session keySound:1] && ![session commitSound] && ![session setMusicActive:YES]);
        assert([session closeWithError:&error]);
        assert(![session keySound:0] && ![session commitSound] && ![session setMusicActive:NO]);
        assert(![session resetCacheWithError:&error]);
        assert(![session setChinesePunctuationEnabled:NO error:&error]);
        assert(![session viewWithError:&error]);
        assert(error);
        [[NSFileManager defaultManager] removeItemAtPath:root error:nil];

        // An incomplete or unmatched special-mode input shows its raw text as the one Fallback candidate (source 9), and Space commits it, bare Y/R included, as in the reference's PrepareCandidateList (add_local_fallback_candidate in crates/engine/src/session/candidates.rs). Temporary English and Japanese stay off unless their resource files exist, so this session gets placeholders.
        NSString *localRoot = [NSTemporaryDirectory() stringByAppendingPathComponent:NSUUID.UUID.UUIDString];
        NSMutableDictionary *localOptions = [options mutableCopy];
        for (NSString *name in @[@"resources", @"user_data", @"cache", @"dictionaries"]) {
            NSString *path = [localRoot stringByAppendingPathComponent:name];
            BOOL created = [[NSFileManager defaultManager] createDirectoryAtPath:path withIntermediateDirectories:YES attributes:nil error:nil];
            assert(created);
            localOptions[name] = path;
        }
        for (NSString *name in @[@"msime-english.db", @"msime-japanese.dat"]) {
            BOOL written = [[@"fixture" dataUsingEncoding:NSUTF8StringEncoding] writeToFile:[localOptions[@"resources"] stringByAppendingPathComponent:name] atomically:YES];
            assert(written);
        }
        error = nil;
        MSIMEClientSession *local = [[MSIMEClientSession alloc] initWithOptions:localOptions error:&error];
        assert(local && !error);
        assert([local setFocused:YES error:&error]);
        const auto showsOnlyFallback = [](NSDictionary *transition, NSString *text) {
            NSArray *candidates = transition[@"view"][@"candidates"];
            return [candidates isKindOfClass:NSArray.class] && candidates.count == 1 &&
                   [candidates[0][@"text"] isEqual:text] && [candidates[0][@"source"] isEqual:@9];
        };
        assert(showsOnlyFallback([local typeASCII:'Y' shift:YES error:&error], @"Y"));
        NSDictionary *bareEnglish = [local command:MSIME_COMMIT_CANDIDATE error:&error];
        assert([bareEnglish[@"commit"] isEqual:@"Y"] && [bareEnglish[@"view"][@"candidates"] count] == 0);
        NSDictionary *unmatched = nil;
        assert([local typeASCII:'T' shift:YES error:&error]);
        for (uint8_t key : {'x', 'i', 'n'}) unmatched = [local typeASCII:key shift:NO error:&error];
        assert(showsOnlyFallback(unmatched, @"Txin"));
        assert([[local command:MSIME_COMMIT_CANDIDATE error:&error][@"commit"] isEqual:@"Txin"]);
        assert(showsOnlyFallback([local typeASCII:'R' shift:YES error:&error], @"R"));
        assert([[local command:MSIME_COMMIT_CANDIDATE error:&error][@"commit"] isEqual:@"R"]);
        assert([local closeWithError:&error]);
        [[NSFileManager defaultManager] removeItemAtPath:localRoot error:nil];

        // 生命周期核心经包装层的往返：选择加入 FEEDBACK 之后，按键特效来自 lifecycleKey:，上屏特效写进派发回复的 feedback，失焦回复不带它，旧的声音和特效调用（事件 0-4）都成了空操作。按键音和上屏音开着、音量为 0：旧调用在选择加入之前真的把请求排进播放器队列（答 YES），之后答 NO，断言才分得出 FEEDBACK 的过滤；音量 0 不会发出任何声音。
        NSString *lifecycleRoot = [NSTemporaryDirectory() stringByAppendingPathComponent:NSUUID.UUID.UUIDString];
        NSMutableDictionary *lifecycleOptions = [options mutableCopy];
        for (NSString *name in @[@"resources", @"user_data", @"cache", @"dictionaries"]) {
            NSString *path = [lifecycleRoot stringByAppendingPathComponent:name];
            BOOL created = [[NSFileManager defaultManager] createDirectoryAtPath:path withIntermediateDirectories:YES attributes:nil error:nil];
            assert(created);
            lifecycleOptions[name] = path;
        }
        NSMutableDictionary *lifecyclePreferences = [options[@"preferences"] mutableCopy];
        lifecyclePreferences[@"plugins"] = @{@"effect_style": @"flash", @"combo_counter": @YES,
                                             @"key_sound": @{@"enabled": @YES, @"volume": @0}, @"commit_sound": @{@"enabled": @YES}};
        lifecycleOptions[@"preferences"] = lifecyclePreferences;
        error = nil;
        MSIMEClientSession *lifecycle = [[MSIMEClientSession alloc] initWithOptions:lifecycleOptions error:&error];
        assert(lifecycle && !error);
        assert([lifecycle setFocused:YES error:&error]);
        const uint32_t flash = 1u << 17;
        assert(lifecycle.lifecycleFlags == 0 && [lifecycle lifecycleKey:0] == 0 && [lifecycle setFieldTraits:MSIME_TRAIT_KNOWN] == 0);
        assert([lifecycle typingEffect:0] == (flash | 1));
        // 选择加入之前旧的声音调用照常排队。
        assert([lifecycle keySound:0] && [lifecycle commitSound] && [lifecycle setMusicActive:YES]);
        assert([lifecycle setLifecycle:MSIME_LIFECYCLE_FEEDBACK | 0x40000000u] == MSIME_LIFECYCLE_FEEDBACK);
        assert(lifecycle.lifecycleFlags == MSIME_LIFECYCLE_FEEDBACK);
        assert([lifecycle setFieldTraits:MSIME_TRAIT_KNOWN] == MSIME_TRAIT_KNOWN);
        assert([lifecycle setLifecycleActive:YES]);
        for (uint32_t event = 0; event <= 4; ++event) assert([lifecycle typingEffect:event] == 0);
        assert(![lifecycle keySound:0] && ![lifecycle commitSound] && ![lifecycle setMusicActive:YES]);
        // 事件 5（绕过按键音的删除）仍由宿主调用：它结束连击，只报告样式。
        assert([lifecycle typingEffect:5] == flash);
        assert([lifecycle lifecycleKey:0] == (flash | 1));
        NSDictionary *typed = [lifecycle typeASCII:'U' shift:YES error:&error];
        assert([typed[@"feedback"][@"typing_effect"] isEqual:@0]);
        uint32_t combo = 1;
        for (uint8_t character : {'4', 'e', '2', 'd'}) {
            assert([lifecycle lifecycleKey:0] == (flash | ++combo));
            typed = [lifecycle typeASCII:character shift:NO error:&error];
            assert(typed && [typed[@"feedback"][@"typing_effect"] isEqual:@0]);
        }
        assert([lifecycle lifecycleKey:1] == (flash | 6));
        NSDictionary *committed = [lifecycle command:MSIME_COMMIT_CANDIDATE error:&error];
        assert([committed[@"commit"] isEqual:@"中"]);
        assert([committed[@"feedback"][@"typing_effect"] isEqual:@(flash | 6)]);
        // 指针调用不出声不画；SECURE 字段里什么都不出，上屏的 feedback 也是 0。
        assert([lifecycle lifecycleKey:MSIME_KEY_POINTER] == 0);
        assert([lifecycle setFieldTraits:MSIME_TRAIT_KNOWN | MSIME_TRAIT_SECURE] == (MSIME_TRAIT_KNOWN | MSIME_TRAIT_SECURE));
        assert([lifecycle lifecycleKey:0] == 0);
        assert([lifecycle typeASCII:'U' shift:YES error:&error]);
        for (uint8_t character : {'4', 'e', '2', 'd'}) assert([lifecycle typeASCII:character shift:NO error:&error]);
        assert([[lifecycle command:MSIME_COMMIT_CANDIDATE error:&error][@"feedback"][@"typing_effect"] isEqual:@0]);
        assert([lifecycle setFieldTraits:MSIME_TRAIT_KNOWN] == MSIME_TRAIT_KNOWN);
        // 宿主自己写进文档之后丢弃组字用命令 19，对会话同 MSIME_CANCEL。
        assert([lifecycle typeASCII:'U' shift:YES error:&error]);
        NSDictionary *hostCommitted = [lifecycle command:MSIME_CANCEL_AFTER_HOST_COMMIT error:&error];
        assert(hostCommitted && ![hostCommitted[@"commit"] isKindOfClass:NSString.class] && [hostCommitted[@"view"][@"editing_text"] length] == 0);
        assert([hostCommitted[@"feedback"][@"typing_effect"] isEqual:@0]);
        // 失焦的回复不带 feedback：失焦上屏不出声也不画。
        NSDictionary *blurred = [lifecycle setFocused:NO error:&error];
        assert([blurred isKindOfClass:NSDictionary.class]);
        assert(blurred && blurred[@"feedback"] == nil);
        // 零拷贝 C 快照路径：通过 ShurufaViewSnapshot / ShurufaTransitionSnapshot 完成聚焦、打字、翻页、选词上屏。
        assert([lifecycle setFocusedSnapshot:YES error:&error] != nullptr);
        const ShurufaTransitionSnapshot *typedSnap = nullptr;
        for (uint8_t character : {'U', '4', 'e', '2', 'd'}) {
            typedSnap = [lifecycle typeASCIISnapshot:character shift:(character == 'U') error:&error];
            assert(typedSnap && typedSnap->handled && typedSnap->view.editing_text.len > 0);
        }
        const ShurufaViewSnapshot *currentView = [lifecycle viewSnapshotWithError:&error];
        assert(currentView && currentView->candidates_len > 0 && currentView->focused);
        assert([lifecycle commandSnapshot:MSIME_NEXT_PAGE error:&error] != nullptr || !error);
        currentView = [lifecycle viewSnapshotWithError:&error];
        const ShurufaTransitionSnapshot *selSnap = [lifecycle selectSnapshotGeneration:currentView->generation index:0 error:&error];
        assert(selSnap && selSnap->has_commit && selSnap->commit.len > 0);
        SmokeClient *snapClient = [SmokeClient new];
        MSIMEApplyTransitionSnapshotWithCommitObserver(selSnap, nil, snapClient, MSIMEInlinePreeditStylePinyin, nil, nullptr, nil, nil);
        assert([snapClient.committed isEqual:@"中"]);
        // 前台应用场景桶同步与安全域保护：常规应用允许写入，进入 SECURE 后强制拒绝并退回 general。
        assert([lifecycle setAppBundleID:@"com.microsoft.VSCode" error:&error] && !error);
        assert([lifecycle setFieldTraits:MSIME_TRAIT_KNOWN | MSIME_TRAIT_SECURE] == (MSIME_TRAIT_KNOWN | MSIME_TRAIT_SECURE));
        error = nil;
        assert(![lifecycle setAppBundleID:@"com.tencent.xinWeChat" error:&error] && !error);
        assert([lifecycle setFieldTraits:MSIME_TRAIT_KNOWN] == MSIME_TRAIT_KNOWN);
        assert([lifecycle setAppBundleID:@"com.tencent.xinWeChat" error:&error] && !error);
        assert([lifecycle setAppBundleID:nil error:&error] && !error);
        assert([lifecycle setLifecycleActive:NO]);
        assert([lifecycle closeWithError:&error]);
        assert([lifecycle lifecycleKey:0] == 0 && ![lifecycle setLifecycleActive:YES] && [lifecycle setFieldTraits:MSIME_TRAIT_KNOWN] == 0 && [lifecycle setLifecycle:MSIME_LIFECYCLE_FEEDBACK] == 0);
        [[NSFileManager defaultManager] removeItemAtPath:lifecycleRoot error:nil];
        puts("Apple Foundation consumer: input, commit, lifecycle, thread and lifetime checks passed");
    }
    return 0;
}
