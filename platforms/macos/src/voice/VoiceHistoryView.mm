#import "VoiceHistoryView.h"
#import <AVFoundation/AVFoundation.h>
#import "VoiceArchive.h"
#import "VoiceRecognitionQuery.h"
#import "VoiceProviderSocket.h"
#import "LocalVoiceRequest.h"
#import "HTTPVoiceRequest.h"
#import "DoubaoVoiceRequest.h"
#import "MSIMEClientSession.h"
#import "../settings/SettingsLayout.h"
#import "../settings/RuntimeOptions.h"

namespace {
constexpr NSUInteger PageSize = 50;
/// 每次送给识别服务 0.1 秒（1600 个 f32 样本）。
constexpr NSUInteger ChunkBytes = 1600 * sizeof(float);
constexpr int64_t RetranscriptionTimeout = 180;
NSArray<NSNumber *> *RecordingHours() { return @[@24, @72]; }
NSArray<NSString *> *RecordingTitles() { return @[@"24 小时", @"3 天"]; }
NSArray<NSNumber *> *HistoryDays() { return @[@0, @30, @90, @365]; }
NSArray<NSString *> *HistoryTitles() { return @[@"永久", @"30 天", @"90 天", @"1 年"]; }

NSString *Describe(NSError *error) {
    NSString *detail = error.userInfo[NSLocalizedFailureReasonErrorKey];
    return [detail isKindOfClass:NSString.class] && [detail length] ? detail : error.localizedDescription ?: @"识别失败";
}

NSString *StatusTitle(NSString *status) {
    NSDictionary *titles = @{@"recording": @"正在录音", @"recognized": @"已识别", @"failed": @"失败",
                             @"cancelled": @"已取消", @"interrupted": @"中断", @"no_result": @"无结果"};
    return titles[status] ?: status;
}

NSString *ApplicationName(NSString *bundleIdentifier) {
    static NSMutableDictionary<NSString *, NSString *> *cache = [NSMutableDictionary dictionary];
    if (!bundleIdentifier.length) return nil;
    if (NSString *cached = cache[bundleIdentifier]) return cached.length ? cached : bundleIdentifier;
    NSURL *url = [NSWorkspace.sharedWorkspace URLForApplicationWithBundleIdentifier:bundleIdentifier];
    NSString *name = url ? [[NSFileManager.defaultManager displayNameAtPath:url.path] stringByDeletingPathExtension] : @"";
    cache[bundleIdentifier] = name ?: @"";
    return name.length ? name : bundleIdentifier;
}

NSString *TimeTitle(NSNumber *milliseconds) {
    NSDate *date = [NSDate dateWithTimeIntervalSince1970:milliseconds.doubleValue / 1000.0];
    NSDateFormatter *formatter = [NSDateFormatter new];
    formatter.locale = [NSLocale localeWithLocaleIdentifier:@"zh_CN"];
    formatter.dateFormat = [NSCalendar.currentCalendar isDateInToday:date] ? @"今天 HH:mm" : @"M月d日 HH:mm";
    return [formatter stringFromDate:date];
}

NSString *DurationTitle(NSNumber *milliseconds) {
    const long seconds = lround(milliseconds.doubleValue / 1000.0);
    return seconds >= 60 ? [NSString stringWithFormat:@"%ld 分 %ld 秒", seconds / 60, seconds % 60]
                         : [NSString stringWithFormat:@"%ld 秒", MAX(seconds, 1L)];
}

/// 把补丁写进共享偏好文档的 voice_input，再镜像到 NSUserDefaults；与语音设置页保存快捷键是同一条路径。
BOOL SaveVoicePatch(NSDictionary *patch) {
    NSString *directory = MSIMELoadRuntimeOptions()[@"preferences_directory"];
    if (![directory isKindOfClass:NSString.class] || !directory.isAbsolutePath) return NO;
    NSDictionary *snapshot = [MSIMEClientSession loadPreferencesInDirectory:directory error:nil];
    NSMutableDictionary *preferences = [snapshot[@"preferences"] mutableCopy];
    NSMutableDictionary *voice = [preferences[@"voice_input"] mutableCopy];
    if (!snapshot || !preferences || !voice) return NO;
    [voice addEntriesFromDictionary:patch];
    preferences[@"voice_input"] = voice;
    NSMutableDictionary *updated = [snapshot mutableCopy];
    updated[@"preferences"] = preferences;
    if (![MSIMEClientSession savePreferencesInDirectory:directory expectedRevision:[snapshot[@"revision"] unsignedLongLongValue]
                                               snapshot:updated error:nil]) return NO;
    MSIMEApplySharedVoicePreferences(voice, NSUserDefaults.standardUserDefaults);
    [NSUserDefaults.standardUserDefaults synchronize];
    return YES;
}

/// 用当前的识别设置把一段存档音频（16 kHz 单声道 f32）再听写一遍，开了整理就照常整理。回调在主线程，只调用一次。
void Retranscribe(NSData *pcm, void (^completion)(NSString *_Nullable text, NSString *_Nullable failure)) {
    if (MSIMEVoiceProviderSocket()) { completion(nil, @"外部识别服务不支持重新听写"); return; }
    NSDictionary *query = MSIMEVoiceRecognitionQuery(NSUserDefaults.standardUserDefaults, 0, NO);
    NSString *provider = query[@"asr_provider"];
    NSError *error = nil;
    if (MSIMEVoiceUsesNativeHTTPProvider(provider, NO)) {
        __block MSIMEHTTPVoiceRequest *request = [[MSIMEHTTPVoiceRequest alloc] initWithOptions:query error:&error];
        if (!request || ![request recognizePCM:pcm completion:^(NSString *text, NSError *failure) {
                request = nil;
                completion(text.length ? text : nil, failure ? Describe(failure) : text.length ? nil : @"未识别到语音");
            } error:&error]) completion(nil, Describe(error));
        return;
    }
    __block id<MSIMEStreamingVoiceRequest> request = nil;
    if ([provider isEqual:@"textstream"])
        request = [[MSIMELocalVoiceRequest alloc] initWithTextStreamOptions:query hostOptions:MSIMELoadRuntimeOptions() error:&error];
    else if ([provider isEqual:@"local"])
        request = [[MSIMELocalVoiceRequest alloc] initWithOptions:query hostOptions:MSIMELoadRuntimeOptions() error:&error];
    else if ([provider isEqual:@"doubao"])
        request = [[MSIMEDoubaoVoiceRequest alloc] initWithOptions:query error:&error];
    else { completion(nil, @"系统语音识别不支持重新听写"); return; }
    if (!request) { completion(nil, Describe(error)); return; }
    __block BOOL done = NO;
    void (^finish)(NSString *, NSString *) = ^(NSString *text, NSString *failure) {
        if (done) return;
        done = YES;
        [request cancel];
        request = nil;
        completion(text, failure);
    };
    MSIMEHTTPVoiceRequest *polisher = ([query[@"polish_enabled"] boolValue] || [query[@"polish_text"] boolValue]) &&
        [query[@"polish_token"] length] ? [[MSIMEHTTPVoiceRequest alloc] initWithPolishOptions:query error:nil] : nil;
    BOOL started = [request startWithResult:^(NSString *text, BOOL final, NSError *failure) {
        dispatch_async(dispatch_get_main_queue(), ^{
            if (done) return;
            if (failure) { finish(nil, Describe(failure)); return; }
            if (!final) return;
            if (!text.length) { finish(nil, @"未识别到语音"); return; }
            // 整理失败或超时时用原文，与录音时一致。
            if (polisher && [polisher polishText:text completion:^(NSString *polished, NSError *polishError) {
                    dispatch_async(dispatch_get_main_queue(), ^{ finish(!polishError && polished.length ? polished : text, nil); });
                } error:nil]) return;
            finish(text, nil);
        });
    } error:&error];
    if (!started) { finish(nil, Describe(error)); return; }
    id<MSIMEStreamingVoiceRequest> feeding = request;
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        NSError *sendError = nil;
        BOOL sent = YES;
        for (NSUInteger offset = 0; sent && offset < pcm.length; offset += ChunkBytes)
            sent = [feeding appendPCM:[pcm subdataWithRange:NSMakeRange(offset, MIN(ChunkBytes, pcm.length - offset))] error:&sendError];
        if (sent) sent = [feeding finishWithError:&sendError];
        if (!sent) dispatch_async(dispatch_get_main_queue(), ^{ finish(nil, Describe(sendError)); });
    });
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, RetranscriptionTimeout * NSEC_PER_SEC), dispatch_get_main_queue(), ^{
        finish(nil, @"重新听写超时");
    });
}
}

@interface MSIMEVoiceHistoryView () <AVAudioPlayerDelegate>
@end

@implementation MSIMEVoiceHistoryView {
    NSStackView *_stack;
    NSPopUpButton *_recordingRetention, *_historyRetention;
    NSTextField *_retentionNotice;
    NSUInteger _limit;
    AVAudioPlayer *_player;
    NSString *_playing;
    NSMutableSet<NSString *> *_retranscribing;
    NSMutableDictionary<NSString *, NSString *> *_failures;
}

- (instancetype)initWithFrame:(NSRect)frameRect {
    if (!(self = [super initWithFrame:frameRect])) return nil;
    self.translatesAutoresizingMaskIntoConstraints = NO;
    _limit = PageSize;
    _retranscribing = [NSMutableSet set];
    _failures = [NSMutableDictionary dictionary];
    _stack = [NSStackView new];
    _stack.orientation = NSUserInterfaceLayoutOrientationVertical;
    _stack.alignment = NSLayoutAttributeLeading;
    _stack.spacing = 10.0;
    _stack.translatesAutoresizingMaskIntoConstraints = NO;
    [self addSubview:_stack];
    [NSLayoutConstraint activateConstraints:@[
        [_stack.leadingAnchor constraintEqualToAnchor:self.leadingAnchor],
        [_stack.trailingAnchor constraintEqualToAnchor:self.trailingAnchor],
        [_stack.topAnchor constraintEqualToAnchor:self.topAnchor],
        [_stack.bottomAnchor constraintEqualToAnchor:self.bottomAnchor],
    ]];
    _recordingRetention = [self popUpWithTitles:RecordingTitles() label:@"录音保留期限" action:@selector(retentionChanged:)];
    _historyRetention = [self popUpWithTitles:HistoryTitles() label:@"听写记录保留期限" action:@selector(retentionChanged:)];
    _retentionNotice = MSIMEDetailLabel(@"");
    _retentionNotice.hidden = YES;
    [NSNotificationCenter.defaultCenter addObserver:self selector:@selector(archiveChanged:)
                                               name:MSIMEVoiceArchiveDidChangeNotification object:nil];
    [self reloadHistory];
    return self;
}

- (void)dealloc { [NSNotificationCenter.defaultCenter removeObserver:self]; }

- (NSPopUpButton *)popUpWithTitles:(NSArray<NSString *> *)titles label:(NSString *)label action:(SEL)action {
    NSPopUpButton *button = [[NSPopUpButton alloc] initWithFrame:NSZeroRect pullsDown:NO];
    [button addItemsWithTitles:titles];
    button.accessibilityLabel = label;
    button.target = self;
    button.action = action;
    return button;
}

- (void)archiveChanged:(NSNotification *)notification {
    (void)notification;
    if (self.window.isVisible) [self reloadHistory];
}

- (void)reloadHistory {
    NSUserDefaults *defaults = NSUserDefaults.standardUserDefaults;
    const NSInteger hours = [defaults integerForKey:@"MSIMEClientVoiceRecordingRetentionHours"];
    [_recordingRetention selectItemAtIndex:hours == 72 ? 1 : 0];
    const NSInteger days = [defaults integerForKey:@"MSIMEClientVoiceHistoryRetentionDays"];
    while (_historyRetention.numberOfItems > (NSInteger)HistoryTitles().count) [_historyRetention removeItemAtIndex:HistoryTitles().count];
    NSUInteger index = [HistoryDays() indexOfObject:@(MAX(days, 0))];
    if (index == NSNotFound) {
        [_historyRetention addItemWithTitle:[NSString stringWithFormat:@"%ld 天", (long)days]];
        index = HistoryTitles().count;
    }
    [_historyRetention selectItemAtIndex:index];

    for (NSView *view in _stack.arrangedSubviews.copy) { [_stack removeArrangedSubview:view]; [view removeFromSuperview]; }
    NSBox *retention = MSIMECardWithViews(@[
        MSIMEPreferenceRow(@"录音保留", _recordingRetention),
        MSIMEPreferenceRow(@"听写记录保留", _historyRetention),
        MSIMEDetailLabel(@"每次录音的音频和识别出的文字都加密保存在本机，密钥在登录钥匙串里，换一台 Mac 读不出来。录音到期只删音频，文字按「听写记录保留」另算。"),
        _retentionNotice,
    ], 0);
    [self addRow:MSIMESectionLabel(@"保留期限")];
    [self addRow:retention];

    MSIMEVoiceArchive *archive = [MSIMEVoiceArchive sharedArchive];
    NSUInteger unreadable = 0;
    NSArray<NSDictionary *> *entries = [archive entriesWithUnreadableCount:&unreadable];
    [self addRow:MSIMESectionLabel(entries.count ? [NSString stringWithFormat:@"听写记录（%lu 条）", (unsigned long)entries.count] : @"听写记录")];
    if (!entries) {
        [self addRow:MSIMEDetailLabel(@"读不到钥匙串里的存档密钥（钥匙串可能还没解锁），语音存档暂时关闭。")];
        return;
    }
    if (unreadable)
        [self addRow:MSIMEDetailLabel([NSString stringWithFormat:@"有 %lu 条记录无法解密：密钥已更换或文件损坏。", (unsigned long)unreadable])];
    if (!entries.count) [self addRow:MSIMEDetailLabel(@"还没有录音。")];
    for (NSDictionary *entry in [entries subarrayWithRange:NSMakeRange(0, MIN(_limit, entries.count))])
        [self addRow:[self cardForEntry:entry]];
    if (entries.count > _limit) {
        NSButton *more = [NSButton buttonWithTitle:[NSString stringWithFormat:@"显示更早的记录（还有 %lu 条）", (unsigned long)(entries.count - _limit)]
                                            target:self action:@selector(showMore:)];
        [self addRow:more];
    }
}

- (void)addRow:(NSView *)view {
    [_stack addArrangedSubview:view];
    [view.widthAnchor constraintEqualToAnchor:_stack.widthAnchor].active = YES;
}

- (NSView *)cardForEntry:(NSDictionary *)entry {
    NSString *identifier = entry[@"id"];
    const BOOL hasAudio = [entry[@"has_audio"] boolValue];
    NSString *status = entry[@"status"];
    NSMutableArray<NSString *> *facts = [NSMutableArray arrayWithObject:TimeTitle(entry[@"started_at_ms"])];
    if ([entry[@"audio_ms"] unsignedLongLongValue]) [facts addObject:DurationTitle(entry[@"audio_ms"])];
    if (NSString *app = ApplicationName(entry[@"app"])) [facts addObject:app];
    [facts addObject:StatusTitle(status)];
    if (!hasAudio) [facts addObject:@"录音已过期"];
    NSTextField *header = [NSTextField labelWithString:[facts componentsJoinedByString:@" · "]];
    header.font = [NSFont systemFontOfSize:12 weight:NSFontWeightMedium];
    header.textColor = [status isEqual:@"failed"] || [status isEqual:@"interrupted"] ? NSColor.systemOrangeColor : NSColor.secondaryLabelColor;
    NSMutableArray<NSView *> *rows = [NSMutableArray arrayWithObject:header];

    NSArray *segments = [entry[@"segments"] isKindOfClass:NSArray.class] ? entry[@"segments"] : @[];
    NSString *text = [segments componentsJoinedByString:@""];
    if (text.length) [rows addObject:[self textLabel:text]];
    NSString *retranscription = [entry[@"retranscription"] isKindOfClass:NSString.class] ? entry[@"retranscription"] : nil;
    if (retranscription.length) [rows addObject:[self textLabel:[@"重新听写：" stringByAppendingString:retranscription]]];
    NSString *failure = [entry[@"failure"] isKindOfClass:NSString.class] ? entry[@"failure"] : nil;
    if (failure.length && !text.length) [rows addObject:MSIMEDetailLabel(failure)];
    if (NSString *latest = _failures[identifier]) {
        NSTextField *label = MSIMEDetailLabel([@"重新听写失败：" stringByAppendingString:latest]);
        label.textColor = NSColor.systemRedColor;
        [rows addObject:label];
    }

    const BOOL busy = [_retranscribing containsObject:identifier];
    const BOOL live = [status isEqual:@"recording"];
    NSButton *play = [self button:[_playing isEqual:identifier] ? @"停止" : @"播放" action:@selector(play:) entry:identifier];
    play.enabled = hasAudio && !live;
    NSButton *copy = [self button:@"复制" action:@selector(copyText:) entry:identifier];
    copy.enabled = text.length || retranscription.length;
    NSButton *again = [self button:busy ? @"听写中…" : @"重新听写" action:@selector(retranscribe:) entry:identifier];
    again.enabled = hasAudio && !busy && !live;
    NSButton *remove = [self button:@"删除" action:@selector(remove:) entry:identifier];
    remove.enabled = !live;
    NSStackView *buttons = [NSStackView stackViewWithViews:@[play, copy, again, remove]];
    buttons.orientation = NSUserInterfaceLayoutOrientationHorizontal;
    buttons.spacing = 8;
    [rows addObject:buttons];
    NSBox *card = MSIMECardWithViews(rows, 6);
    card.accessibilityLabel = [facts componentsJoinedByString:@" "];
    return card;
}

- (NSTextField *)textLabel:(NSString *)text {
    NSTextField *label = [NSTextField wrappingLabelWithString:text];
    label.selectable = YES;
    label.font = [NSFont systemFontOfSize:13];
    return label;
}

- (NSButton *)button:(NSString *)title action:(SEL)action entry:(NSString *)identifier {
    NSButton *button = [NSButton buttonWithTitle:title target:self action:action];
    button.identifier = identifier;
    button.controlSize = NSControlSizeSmall;
    return button;
}

- (NSDictionary *)entryForIdentifier:(NSString *)identifier {
    for (NSDictionary *entry in [[MSIMEVoiceArchive sharedArchive] entriesWithUnreadableCount:nil] ?: @[])
        if ([entry[@"id"] isEqual:identifier]) return entry;
    return nil;
}

- (void)showMore:(id)sender {
    (void)sender;
    _limit += PageSize;
    [self reloadHistory];
}

- (void)retentionChanged:(id)sender {
    (void)sender;
    const NSInteger historyIndex = _historyRetention.indexOfSelectedItem;
    NSNumber *days = historyIndex >= 0 && historyIndex < (NSInteger)HistoryDays().count ? HistoryDays()[historyIndex]
        : @([NSUserDefaults.standardUserDefaults integerForKey:@"MSIMEClientVoiceHistoryRetentionDays"]);
    NSNumber *hours = RecordingHours()[MAX(_recordingRetention.indexOfSelectedItem, 0)];
    if (!SaveVoicePatch(@{@"recording_retention_hours": hours, @"history_retention_days": days})) {
        _retentionNotice.stringValue = @"保留期限未保存，偏好可能已变化，请重新打开设置后重试。";
        _retentionNotice.textColor = NSColor.systemRedColor;
        _retentionNotice.hidden = NO;
        return;
    }
    // 缩短期限立刻生效：马上按新期限清理一次。
    [[MSIMEVoiceArchive sharedArchive] purge];
}

- (void)play:(NSButton *)sender {
    NSString *identifier = sender.identifier;
    const BOOL stopping = [_playing isEqual:identifier];
    [_player stop];
    _player = nil;
    _playing = nil;
    if (!stopping) {
        NSData *wav = [[MSIMEVoiceArchive sharedArchive] WAVForRecording:identifier];
        _player = wav ? [[AVAudioPlayer alloc] initWithData:wav error:nil] : nil;
        _player.delegate = self;
        if ([_player play]) _playing = identifier;
    }
    [self reloadHistory];
}

- (void)audioPlayerDidFinishPlaying:(AVAudioPlayer *)player successfully:(BOOL)flag {
    (void)flag;
    if (player != _player) return;
    _player = nil;
    _playing = nil;
    [self reloadHistory];
}

- (void)copyText:(NSButton *)sender {
    NSDictionary *entry = [self entryForIdentifier:sender.identifier];
    NSArray *segments = [entry[@"segments"] isKindOfClass:NSArray.class] ? entry[@"segments"] : @[];
    NSString *text = [segments componentsJoinedByString:@""];
    if (!text.length && [entry[@"retranscription"] isKindOfClass:NSString.class]) text = entry[@"retranscription"];
    if (!text.length) return;
    [NSPasteboard.generalPasteboard clearContents];
    [NSPasteboard.generalPasteboard setString:text forType:NSPasteboardTypeString];
}

- (void)retranscribe:(NSButton *)sender {
    NSString *identifier = sender.identifier;
    NSData *pcm = [[MSIMEVoiceArchive sharedArchive] PCMForRecording:identifier];
    if (!pcm.length) { _failures[identifier] = @"录音读不出来"; [self reloadHistory]; return; }
    [_retranscribing addObject:identifier];
    [_failures removeObjectForKey:identifier];
    [self reloadHistory];
    __weak MSIMEVoiceHistoryView *weakSelf = self;
    Retranscribe(pcm, ^(NSString *text, NSString *failure) {
        MSIMEVoiceHistoryView *view = weakSelf;
        if (text.length && ![[MSIMEVoiceArchive sharedArchive] noteRetranscription:text forRecording:identifier])
            failure = @"结果没能写进存档";
        if (!view) return;
        [view->_retranscribing removeObject:identifier];
        if (failure) view->_failures[identifier] = failure;
        [view reloadHistory];
    });
}

- (void)remove:(NSButton *)sender {
    NSString *identifier = sender.identifier;
    NSAlert *alert = [NSAlert new];
    alert.messageText = @"删除这条录音和它的文字？";
    alert.informativeText = @"删除后无法恢复。";
    [alert addButtonWithTitle:@"删除"];
    [alert addButtonWithTitle:@"取消"];
    alert.buttons.firstObject.hasDestructiveAction = YES;
    __weak MSIMEVoiceHistoryView *weakSelf = self;
    void (^handler)(NSModalResponse) = ^(NSModalResponse response) {
        MSIMEVoiceHistoryView *view = weakSelf;
        if (response != NSAlertFirstButtonReturn || !view) return;
        if ([view->_playing isEqual:identifier]) { [view->_player stop]; view->_player = nil; view->_playing = nil; }
        [[MSIMEVoiceArchive sharedArchive] deleteRecording:identifier];
        [view reloadHistory];
    };
    if (self.window) [alert beginSheetModalForWindow:self.window completionHandler:handler];
    else handler([alert runModal]);
}
@end
