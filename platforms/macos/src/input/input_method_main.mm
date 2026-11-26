#import <AppKit/AppKit.h>
#import <InputMethodKit/InputMethodKit.h>
#import "InputModeIdentifiers.h"
#import "InputModeStatusItem.h"
#import "InputSourceRegistration.h"
#import "InputControllerActivity.h"
#import "../settings/PreferencesWindowController.h"
#import "../settings/RuntimeOptions.h"
#import "../settings/RuntimeOptionsRefresh.h"
#import "../settings/AppearancePreferences.h"
#import "MSIMEClientSession.h"
#import "../candidate/CandidateSkin.h"
#import "../voice/VoiceAudioMuter.h"
#import "../voice/VoiceArchive.h"
#import "../voice/VoiceInputService.h"
#include "../core/DiagnosticLog.h"
#import "../core/InstalledHost.h"
#import "../settings/ShurufaPreferenceMigration.h"
#include "msime_client.h"
#include <cstdio>
#include <cstring>

static bool MSIMEShouldShowPreferences(int argc, const char *argv[]) {
    for (int index = 1; index < argc; ++index) {
        if (strcmp(argv[index], "--preferences") == 0) return true;
    }
    return false;
}

static void MSIMEConfigureMovableState(void) {
    NSDictionary *options = MSIMELoadRuntimeOptions();
    NSString *directory = [options[@"preferences_directory"] isKindOfClass:NSString.class]
        ? options[@"preferences_directory"] : nil;
    if (directory.length > 0 && directory.isAbsolutePath) {
        metasequoia::mac::SetDefaultSkinsRoot(
            std::filesystem::path(directory.fileSystemRepresentation) / "skins");
    } else if (!MSIMEEditionIsFull()) {
        // 没有配置状态目录时 CandidateSkin.cpp 退回 full 的状态目录；其他版本的皮肤在自己的状态目录下。
        NSURL *state = MSIMEDefaultClientStateDirectory(NSFileManager.defaultManager);
        if (state.path.length > 0)
            metasequoia::mac::SetDefaultSkinsRoot(std::filesystem::path(state.path.fileSystemRepresentation) / "skins");
    }
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        for (int index = 1; index < argc; ++index) {
            if (strcmp(argv[index], "--describe-host") != 0) continue;
            NSDictionary *identity = @{
                @"bundle_id": MSIMEInputMethodBundleIdentifier(),
                @"edition": MSIMEEditionIdentifier(),
                @"state_directory": MSIMEDefaultClientStateDirectory(NSFileManager.defaultManager).path,
                @"preferences_domain": MSIMEInputMethodBundleIdentifier(),
                @"settings_bundle_id": MSIMESettingsBundleIdentifier(),
                @"account_keychain": MSIMEKeychainService(),
                @"voice_keychain": MSIMEVoiceProviderKeychainService(),
                @"voice_helper": MSIMELocalVoiceHelperName(),
                @"dictionary_notification": MSIMEEditionNotificationName(@"MSIMEDictionaryMaintenanceWillBeginNotification"),
                @"statistics_notification": MSIMEEditionNotificationName(@"MetasequoiaTypingStatisticsEnabledChangedNotification")
            };
            NSData *data = [NSJSONSerialization dataWithJSONObject:identity options:NSJSONWritingSortedKeys error:nil];
            if (!data) return 1;
            std::fwrite(data.bytes, 1, data.length, stdout);
            std::putchar('\n');
            return 0;
        }
        if (MSIMEShouldReportInputSourceRegistration(argc, argv)) {
            return MSIMEInputSourceRegistryExitCode(
                MSIMEInputSourceRegistryStateFor(NSBundle.mainBundle.bundleIdentifier, TISCreateInputSourceList));
        }
        if (MSIMEShouldReportEnabledInputModes(argc, argv)) {
            NSArray<NSString *> *modes = MSIMEEnabledInputModeIDs(NSBundle.mainBundle.bundleIdentifier,
                TISCreateInputSourceList,
                [](TISInputSourceRef source, CFStringRef key) -> void * {
                    return (void *)TISGetInputSourceProperty(source, key);
                });
            if (!modes) return 1;
            for (NSString *mode in modes) std::printf("%s\n", mode.UTF8String);
            return 0;
        }
        if (argc == 2 && std::strcmp(argv[1], "--announce-input-source-change") == 0) {
            // 重装时 LaunchServices 先报注销再报登记，菜单栏输入法菜单会把 Shurufa 去掉而不再加回；
            // 系统设置添加输入法后也是靠这两条通知让菜单重读。这里只通知，不改启用状态、不切输入源。
            for (NSString *name in @[@"AppleInstalledInputSourcesChangedNotification", @"AppleEnabledInputSourcesChangedNotification"])
                [NSDistributedNotificationCenter.defaultCenter postNotificationName:name object:nil userInfo:nil deliverImmediately:YES];
            return 0;
        }
        if (MSIMEShouldReportInputSourceEnabled(argc, argv)) {
            std::printf("%s\n", MSIMEInputSourceIsEnabled(NSBundle.mainBundle.bundleIdentifier) ? "true" : "false");
            return 0;
        }
        if (!MSIMEHostCanServeIn(MSIMEEditionInfo(), NSBundle.mainBundle.bundlePath, NSHomeDirectory())) {
            MSIMEUnregisterUninstalledHost();
            std::fprintf(stderr, "Shurufa: install the bundle before starting the input method\n");
            return 78;
        }
        if (MSIMEEditionIsShurufa()) NSLog(@"Shurufa installed-host accepted: %@", NSBundle.mainBundle.bundlePath);
        if (MSIMEShouldRegisterInputSource(argc, argv)) {
            NSURL *bundleURL = NSBundle.mainBundle.bundleURL;
            NSString *identifier = NSBundle.mainBundle.bundleIdentifier;
            OSStatus status = MSIMERegisterAndEnableInputSources(bundleURL, identifier,
                TISRegisterInputSource, TISCreateInputSourceList,
                [](TISInputSourceRef source, CFStringRef key) -> void * {
                    return (void *)TISGetInputSourceProperty(source, key);
                }, TISEnableInputSource);
            return status == noErr ? 0 : 1;
        }
        if (argc == 2 && std::strcmp(argv[1], "--ensure-input-source-enabled") == 0) {
            if (!MSIMEEditionIsShurufa()) return 1;
            NSURL *bundleURL = NSBundle.mainBundle.bundleURL;
            NSString *identifier = NSBundle.mainBundle.bundleIdentifier;
            NSString *primaryMode = [identifier stringByAppendingString:@".Hans"];
            NSString *modePrefix = [identifier stringByAppendingString:@"."];
            const OSStatus regStatus = TISRegisterInputSource((__bridge CFURLRef)bundleURL);
            if (regStatus != noErr) return 1;
            // 检查实时已启用列表（includeAllInstalled = false）：若父容器与至少一个模式均已在线启用，不作任何改动。
            NSDictionary *bundleFilter = @{(__bridge NSString *)kTISPropertyBundleID: identifier};
            BOOL parentLive = NO;
            BOOL modeLive = NO;
            CFArrayRef liveList = TISCreateInputSourceList((__bridge CFDictionaryRef)bundleFilter, false);
            if (liveList) {
                for (CFIndex i = 0; i < CFArrayGetCount(liveList); ++i) {
                    TISInputSourceRef src = (TISInputSourceRef)CFArrayGetValueAtIndex(liveList, i);
                    void *prop = TISGetInputSourceProperty(src, kTISPropertyInputSourceID);
                    if (!prop || CFGetTypeID(prop) != CFStringGetTypeID()) continue;
                    NSString *sid = (__bridge NSString *)prop;
                    if ([sid isEqualToString:identifier]) parentLive = YES;
                    else if ([sid hasPrefix:modePrefix]) modeLive = YES;
                }
                CFRelease(liveList);
            }
            if (parentLive && modeLive) return 0;
            // 仅在父容器被系统停用或尚无任何模式在线启用时补齐启用（永不调用 TISDisableInputSource，永不切换当前输入源）。
            CFArrayRef installed = TISCreateInputSourceList((__bridge CFDictionaryRef)bundleFilter, true);
            if (!installed || CFArrayGetCount(installed) == 0) {
                if (installed) CFRelease(installed);
                return 1;
            }
            NSDictionary *declaredDict = NSBundle.mainBundle.infoDictionary[@"ComponentInputModeDict"][@"tsInputModeListKey"];
            NSSet<NSString *> *declaredModes = [declaredDict isKindOfClass:NSDictionary.class]
                ? [NSSet setWithArray:declaredDict.allKeys] : [NSSet set];
            TISInputSourceRef parentSource = nullptr;
            TISInputSourceRef primarySource = nullptr;
            NSMutableArray *enabledModes = [NSMutableArray array];
            for (CFIndex i = 0; i < CFArrayGetCount(installed); ++i) {
                TISInputSourceRef src = (TISInputSourceRef)CFArrayGetValueAtIndex(installed, i);
                void *prop = TISGetInputSourceProperty(src, kTISPropertyInputSourceID);
                if (!prop || CFGetTypeID(prop) != CFStringGetTypeID()) continue;
                NSString *sid = (__bridge NSString *)prop;
                if ([sid isEqualToString:identifier]) {
                    parentSource = src;
                } else if ([sid hasPrefix:modePrefix] && [declaredModes containsObject:sid]) {
                    if ([sid isEqualToString:primaryMode]) primarySource = src;
                    void *en = TISGetInputSourceProperty(src, kTISPropertyInputSourceIsEnabled);
                    if (en && CFGetTypeID(en) == CFBooleanGetTypeID() && CFBooleanGetValue((CFBooleanRef)en)) {
                        [enabledModes addObject:(__bridge id)src];
                    }
                }
            }
            if (!parentSource) {
                CFRelease(installed);
                return 1;
            }
            OSStatus status = TISEnableInputSource(parentSource);
            if (status == noErr) {
                if (enabledModes.count > 0) {
                    for (id obj in enabledModes) {
                        const OSStatus rc = TISEnableInputSource((__bridge TISInputSourceRef)obj);
                        if (rc != noErr) status = rc;
                    }
                } else if (primarySource) {
                    status = TISEnableInputSource(primarySource);
                } else {
                    status = fnfErr;
                }
            }
            CFRelease(installed);
            for (NSString *name in @[@"AppleInstalledInputSourcesChangedNotification", @"AppleEnabledInputSourcesChangedNotification"]) {
                [NSDistributedNotificationCenter.defaultCenter postNotificationName:name object:nil userInfo:nil deliverImmediately:YES];
            }
            return status == noErr ? 0 : 1;
        }
        if (MSIMEShouldRefreshInstalledInputSources(argc, argv)) {
            if (!MSIMEEditionIsShurufa()) return 1;
            NSDictionary *modes = NSBundle.mainBundle.infoDictionary[@"ComponentInputModeDict"][@"tsInputModeListKey"];
            NSArray *previous = [[NSString stringWithUTF8String:argv[2]] componentsSeparatedByString:@"\n"];
            OSStatus status = MSIMERefreshInstalledInputSources(NSBundle.mainBundle.bundleURL, NSBundle.mainBundle.bundleIdentifier,
                modes.allKeys, previous, TISRegisterInputSource, TISCreateInputSourceList,
                [](TISInputSourceRef source, CFStringRef key) -> void * { return (void *)TISGetInputSourceProperty(source, key); },
                TISEnableInputSource, TISDisableInputSource);
            return status == noErr ? 0 : 1;
        }
        NSString *prepareFailure = nil;
        if (!MSIMEPrepareShurufaHost(&prepareFailure)) {
            // launchd 启动的实例 stderr 不落盘，同时写统一日志留下痕迹。
            NSLog(@"Shurufa prepare-host failed: %@", prepareFailure);
            std::fprintf(stderr, "Shurufa: cannot prepare bundled input resources: %s\n", prepareFailure.UTF8String ?: "");
            return 1;
        }
        for (int index = 1; index < argc; ++index) {
            if (strcmp(argv[index], "--migrate-quanpin-helpcode") == 0) {
                // 全拼辅助码已在核心中彻底移除，保留该启动参数为空操作以兼容升级脚本。
                return 0;
            }
            if (strcmp(argv[index], "--migrate-grammar-model") == 0) {
                if (!MSIMEEditionIsShurufa()) return 1;
                NSString *directory = MSIMEDefaultClientStateDirectory(NSFileManager.defaultManager).path;
                const BOOL migrated = MSIMEMigrateShurufaGrammarModel(NSUserDefaults.standardUserDefaults,
                    ^NSDictionary *{ return [MSIMEClientSession loadPreferencesInDirectory:directory error:nil]; },
                    ^NSDictionary *(uint64_t revision, NSDictionary *snapshot) {
                        return [MSIMEClientSession savePreferencesInDirectory:directory expectedRevision:revision snapshot:snapshot error:nil];
                    });
                return migrated ? 0 : 1;
            }
            if (strcmp(argv[index], "--migrate-inline-emoji") == 0) {
                if (!MSIMEEditionIsShurufa()) return 1;
                NSString *directory = MSIMEDefaultClientStateDirectory(NSFileManager.defaultManager).path;
                const BOOL migrated = MSIMEMigrateShurufaInlineEmoji(NSUserDefaults.standardUserDefaults,
                    ^NSDictionary *{ return [MSIMEClientSession loadPreferencesInDirectory:directory error:nil]; },
                    ^NSDictionary *(uint64_t revision, NSDictionary *snapshot) {
                        return [MSIMEClientSession savePreferencesInDirectory:directory expectedRevision:revision snapshot:snapshot error:nil];
                    });
                return migrated ? 0 : 1;
            }
            if (strcmp(argv[index], "--migrate-voice-presentation") == 0) {
                if (!MSIMEEditionIsShurufa()) return 1;
                NSString *directory = MSIMEDefaultClientStateDirectory(NSFileManager.defaultManager).path;
                const BOOL migrated = MSIMEMigrateShurufaVoicePresentation(NSUserDefaults.standardUserDefaults,
                    ^NSDictionary *{ return [MSIMEClientSession loadPreferencesInDirectory:directory error:nil]; },
                    ^NSDictionary *(uint64_t revision, NSDictionary *snapshot) {
                        return [MSIMEClientSession savePreferencesInDirectory:directory expectedRevision:revision snapshot:snapshot error:nil];
                    });
                return migrated && MSIMEPrepareShurufaHost() ? 0 : 1;
            }
            if (strcmp(argv[index], "--migrate-voice-shortcut") == 0) {
                if (!MSIMEEditionIsShurufa()) return 1;
                NSString *directory = MSIMEDefaultClientStateDirectory(NSFileManager.defaultManager).path;
                return MSIMEMigrateShurufaVoiceShortcut(NSUserDefaults.standardUserDefaults,
                    ^NSDictionary *{ return [MSIMEClientSession loadPreferencesInDirectory:directory error:nil]; },
                    ^NSDictionary *(uint64_t revision, NSDictionary *snapshot) {
                        return [MSIMEClientSession savePreferencesInDirectory:directory expectedRevision:revision snapshot:snapshot error:nil];
                    }) ? 0 : 1;
            }
            if (strcmp(argv[index], "--migrate-caps-lock") == 0) {
                if (!MSIMEEditionIsShurufa()) return 1;
                NSString *directory = MSIMEDefaultClientStateDirectory(NSFileManager.defaultManager).path;
                const BOOL migrated = MSIMEMigrateShurufaCapsLock(NSUserDefaults.standardUserDefaults,
                    ^NSDictionary *{ return [MSIMEClientSession loadPreferencesInDirectory:directory error:nil]; },
                    ^NSDictionary *(uint64_t revision, NSDictionary *snapshot) {
                        return [MSIMEClientSession savePreferencesInDirectory:directory expectedRevision:revision snapshot:snapshot error:nil];
                    });
                return migrated && MSIMEPrepareShurufaHost() ? 0 : 1;
            }
            if (strcmp(argv[index], "--prepare-host") == 0) return 0;
        }
        [NSApplication sharedApplication];
        // Host API reports failures it recovers from by itself (a sound, music or helpcode pack that does not load, an audio device that does not open) as one line on stderr, which launchd points at /dev/null for an input method; this sends them to diagnostic.log instead. Before the first session, whose creation reports the helpcode fallback; lines that arrive before the log is configured or while it is off are dropped, not kept for later (DiagnosticLog.h).
        msime_client_set_diagnostic_sink(msime_macos_diagnostic_host_line);
        // After an app upgrade the options still point at the previous dictionary generation; bring them to the installed one (replaying the user dictionary) before any session, including the standalone preferences window's, reads them.
        // A missing file is the not-yet-configured state, which is not a refresh failure.
        NSString *optionsPath = MSIMERuntimeOptionsPath();
        switch (optionsPath && [NSFileManager.defaultManager fileExistsAtPath:optionsPath]
                    ? MSIMERefreshRuntimeOptions(optionsPath) : MSIMERuntimeOptionsRefreshCurrent) {
        case MSIMERuntimeOptionsRefreshUpdated:
            NSLog(@"MSIME dictionary updated to the installed generation");
            break;
        case MSIMERuntimeOptionsRefreshFailed:
            NSLog(@"MSIME cannot update the dictionary to the installed generation; keeping the current one");
            break;
        case MSIMERuntimeOptionsRefreshCurrent:
            break;
        }
        MSIMEConfigureMovableState();
        // 首次启动时触发单一 `config.json` 读取与幂等迁移，并将已设置过的 `NSUserDefaults` 迁入。
        NSDictionary *startupOptions = MSIMELoadRuntimeOptions();
        id startupPreferences = startupOptions[@"preferences_directory"];
        NSString *startupDirectory = [startupPreferences isKindOfClass:NSString.class] && [startupPreferences isAbsolutePath]
            ? (NSString *)startupPreferences
            : MSIMEDefaultClientStateDirectory(NSFileManager.defaultManager).path;
        if (startupDirectory.length > 0 && startupDirectory.isAbsolutePath) {
            NSUserDefaults *defaults = NSUserDefaults.standardUserDefaults;
            auto loadPrefs = ^NSDictionary *{ return [MSIMEClientSession loadPreferencesInDirectory:startupDirectory error:nil]; };
            auto savePrefs = ^NSDictionary *(uint64_t revision, NSDictionary *snapshot) {
                return [MSIMEClientSession savePreferencesInDirectory:startupDirectory expectedRevision:revision snapshot:snapshot error:nil];
            };
            MSIMEMigrateShurufaCapsLock(defaults, loadPrefs, savePrefs);
            MSIMEMigrateShurufaVoiceShortcut(defaults, loadPrefs, savePrefs);
            MSIMEMigrateShurufaVoicePresentation(defaults, loadPrefs, savePrefs);
            MSIMEMigrateShurufaGrammarModel(defaults, loadPrefs, savePrefs);
            MSIMEMigrateShurufaInlineEmoji(defaults, loadPrefs, savePrefs);
            [MSIMEClientSession loadPreferencesInDirectory:startupDirectory error:nil];
        }
        // 独立设置窗口也要检查模式是否已加入输入法列表，所以在进入该分支前初始化探针。
        MSIMEInputModeEnabledProbe = MSIMEInputSourceIsEnabled;
        if (MSIMEShouldShowPreferences(argc, argv)) {
            [NSApp setActivationPolicy:NSApplicationActivationPolicyRegular];
            if (startupDirectory.length > 0 && startupDirectory.isAbsolutePath)
                [[MSIMEAppearancePreferences sharedPreferences] setTranslationPreferencesDirectory:startupDirectory];
            id closeObserver = [[NSNotificationCenter defaultCenter]
                addObserverForName:MSIMEStandalonePreferencesDidCloseNotification
                            object:nil
                             queue:NSOperationQueue.mainQueue
                        usingBlock:^(NSNotification *notification) {
                            (void)notification;
                            [NSApp terminate:nil];
                        }];
            [[MSIMEPreferencesWindowController sharedController] showAndActivateForStandaloneLaunch];
            [NSApp run];
            [[NSNotificationCenter defaultCenter] removeObserver:closeObserver];
            return 0;
        }
        // Recover a prior crashed capture before accepting new IMK sessions.
        // A running owner holds the journal lock, so this cannot undo its mute.
        [[[MSIMEVoiceAudioMuter alloc] init] restore];
        // 每次录音都进加密存档；打开存档时顺带按保留期限清理一次。
        MSIMEVoiceInputService.archive = [MSIMEVoiceArchive sharedArchive];
        // imklaunchagent 用这个名字找到本 bundle；名字为什么必须是这种形式，见 Info.plist.in。
        NSString *connectionName = NSBundle.mainBundle.infoDictionary[@"InputMethodConnectionName"];
        if (![connectionName isKindOfClass:NSString.class] || connectionName.length == 0) return 1;
        __attribute__((objc_precise_lifetime)) IMKServer *server = [[IMKServer alloc] initWithName:connectionName bundleIdentifier:NSBundle.mainBundle.bundleIdentifier];
        if (!server) return 1;
        // 记录本版本的模式，不自动启用或关闭输入源，保留用户在系统设置中的选择。
        NSString *const offeredModesKey = @"MSIMEOfferedInputModes";
        NSArray *offeredModes = [NSUserDefaults.standardUserDefaults arrayForKey:offeredModesKey];
        NSArray<NSString *> *offered = MSIMEEnableNewInputModes(NSBundle.mainBundle.bundleIdentifier, offeredModes,
            TISCreateInputSourceList,
            [](TISInputSourceRef source, CFStringRef key) -> void * {
                return (void *)TISGetInputSourceProperty(source, key);
            }, TISEnableInputSource);
        if (![offered isEqualToArray:offeredModes]) [NSUserDefaults.standardUserDefaults setObject:offered forKey:offeredModesKey];
        __attribute__((objc_precise_lifetime)) MSIMEInputSourceMonitor *sourceMonitor =
            [[MSIMEInputSourceMonitor alloc] initWithCenter:NSDistributedNotificationCenter.defaultCenter
                bundleIdentifier:NSBundle.mainBundle.bundleIdentifier copySource:TISCopyCurrentKeyboardInputSource
                propertyGetter:[](TISInputSourceRef source, CFStringRef key) -> void * {
                    return (void *)TISGetInputSourceProperty(source, key);
                } switchedAway:^{
                    // Leaving this input method is the counterpart of TSF Deactivate, which clears the open/close, punctuation and width compartments together: every app, in either ime_mode_scope, starts from default_ime_mode and the saved punctuation and width when it comes back. The shared record of the shown mode is cleared too, so picking either mode entry on the way back is adopted as a choice. Focus changes between clients never get here.
                    MSIMEAppearancePreferences *preferences = [MSIMEAppearancePreferences sharedPreferences];
                    if (!MSIMEEditionIsShurufa()) [preferences resetRememberedInputModes];
                    [preferences resetAllRuntimeInputState];
                    MSIMEResetSystemInputModeState(MSIMESharedSystemInputModeState());
                    // 切到 ABC 等别的输入源后，菜单栏状态项不能留着一个过期的「中」；切回来时 activateServer: 的同步会按当前模式重新放上。焦点在客户端之间移动不会走到这里，所以 deactivateServer: 不隐藏它。
                    [[MSIMEInputModeStatusItem sharedItem] hide];
                }];
        [NSApp run];
        [sourceMonitor stop];
        msime_client_release_process_cache();
        (void)server;
    }
    return 0;
}
