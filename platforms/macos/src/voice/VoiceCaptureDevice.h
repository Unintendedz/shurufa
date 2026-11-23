#pragma once
#import <Foundation/Foundation.h>
#import <CoreAudio/CoreAudio.h>
#import <AudioToolbox/AudioToolbox.h>
#include <vector>

// Keep device selection testable without enumerating or opening real hardware.
struct MSIMEVoiceCaptureDeviceAPI {
    decltype(&AudioObjectGetPropertyData) get = AudioObjectGetPropertyData;
    decltype(&AudioObjectGetPropertyDataSize) size = AudioObjectGetPropertyDataSize;
    decltype(&AudioUnitSetProperty) setUnit = AudioUnitSetProperty;
    decltype(&AudioUnitGetProperty) getUnit = AudioUnitGetProperty;
};

/// A stable CoreAudio capture-device identity suitable for preferences.  The
/// UID is intentionally kept separate from the display name: names can be
/// shared by two interfaces and can change when a device is renamed.
struct MSIMEVoiceCaptureDeviceInfo {
    NSString *uid;
    NSString *name;
    BOOL isDefault;
};

struct MSIMEVoiceCaptureDeviceListAPI {
    decltype(&AudioObjectGetPropertyData) get = AudioObjectGetPropertyData;
    decltype(&AudioObjectGetPropertyDataSize) size = AudioObjectGetPropertyDataSize;
};

/// Enumerate input-capable devices without opening them.  The first item is
/// always the current system default, followed by a deterministic name/UID
/// order so a popup does not jump when CoreAudio returns devices differently.
static inline NSArray<NSDictionary *> *MSIMEListVoiceCaptureDevices(
    const MSIMEVoiceCaptureDeviceListAPI &api = {}) {
    AudioObjectPropertyAddress devicesAddress = { kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain };
    UInt32 bytes = 0;
    if (api.size(kAudioObjectSystemObject, &devicesAddress, 0, nullptr, &bytes) != noErr ||
        bytes == 0 || bytes % sizeof(AudioDeviceID) != 0) return @[];
    NSMutableArray<NSDictionary *> *devices = [NSMutableArray array];
    const NSUInteger count = bytes / sizeof(AudioDeviceID);
    std::vector<AudioDeviceID> ids(count);
    if (api.get(kAudioObjectSystemObject, &devicesAddress, 0, nullptr, &bytes, ids.data()) != noErr ||
        bytes != count * sizeof(AudioDeviceID)) return @[];

    AudioDeviceID defaultID = kAudioObjectUnknown;
    AudioObjectPropertyAddress defaultAddress = { kAudioHardwarePropertyDefaultInputDevice,
        kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain };
    UInt32 defaultBytes = sizeof(defaultID);
    (void)api.get(kAudioObjectSystemObject, &defaultAddress, 0, nullptr, &defaultBytes, &defaultID);

    for (AudioDeviceID device : ids) {
        AudioObjectPropertyAddress streamsAddress = { kAudioDevicePropertyStreams,
            kAudioDevicePropertyScopeInput, kAudioObjectPropertyElementMain };
        UInt32 streamBytes = 0;
        if (api.size(device, &streamsAddress, 0, nullptr, &streamBytes) != noErr ||
            streamBytes < sizeof(AudioStreamID) || streamBytes % sizeof(AudioStreamID) != 0) continue;
        AudioObjectPropertyAddress uidAddress = { kAudioDevicePropertyDeviceUID,
            kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain };
        CFStringRef uidValue = nullptr;
        UInt32 uidBytes = sizeof(uidValue);
        if (api.get(device, &uidAddress, 0, nullptr, &uidBytes, &uidValue) != noErr ||
            uidBytes != sizeof(uidValue) || !uidValue) continue;
        NSString *uid = [(__bridge NSString *)uidValue copy];
        if (!uid.length) continue;
        AudioObjectPropertyAddress nameAddress = { kAudioObjectPropertyName,
            kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain };
        CFStringRef nameValue = nullptr;
        UInt32 nameBytes = sizeof(nameValue);
        NSString *name = nil;
        if (api.get(device, &nameAddress, 0, nullptr, &nameBytes, &nameValue) == noErr &&
            nameBytes == sizeof(nameValue) && nameValue) name = [(__bridge NSString *)nameValue copy];
        if (!name.length) name = @"未命名录音设备";
        [devices addObject:@{ @"uid": uid, @"name": name,
                              @"default": @(device == defaultID) }];
    }
    [devices sortUsingComparator:^NSComparisonResult(NSDictionary *left, NSDictionary *right) {
        BOOL leftDefault = [left[@"default"] boolValue], rightDefault = [right[@"default"] boolValue];
        if (leftDefault != rightDefault) return leftDefault ? NSOrderedAscending : NSOrderedDescending;
        NSComparisonResult name = [left[@"name"] localizedStandardCompare:right[@"name"]];
        return name == NSOrderedSame ? [left[@"uid"] compare:right[@"uid"]] : name;
    }];
    return devices;
}

static inline BOOL MSIMEConfigureVoiceCaptureDevice(NSString *uid, AudioUnit unit,
    NSError **error, const MSIMEVoiceCaptureDeviceAPI &api = {}) {
    // Only the absence of an explicit selection opts into the system default.
    if (!uid.length) return YES;
    auto fail = [&](NSInteger code) {
        if (error) *error = [NSError errorWithDomain:@"app.msime.client.voice" code:code
            userInfo:@{NSLocalizedDescriptionKey:@"所选麦克风不可用，请重新选择录音设备"}];
        return NO;
    };
    if (!unit) return fail(20);
    AudioObjectPropertyAddress address = { kAudioHardwarePropertyTranslateUIDToDevice,
        kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain };
    CFStringRef requested = (__bridge CFStringRef)uid;
    AudioDeviceID device = kAudioObjectUnknown;
    UInt32 bytes = sizeof(device);
    if (api.get(kAudioObjectSystemObject, &address, sizeof(requested), &requested, &bytes, &device) != noErr ||
        bytes != sizeof(device) || device == kAudioObjectUnknown) return fail(21);
    // Reject output-only devices before touching the input audio unit.
    address = { kAudioDevicePropertyStreams, kAudioDevicePropertyScopeInput, kAudioObjectPropertyElementMain };
    bytes = 0;
    if (api.size(device, &address, 0, nullptr, &bytes) != noErr ||
        bytes < sizeof(AudioStreamID) || bytes % sizeof(AudioStreamID)) return fail(22);
    if (api.setUnit(unit, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global,
        0, &device, sizeof(device)) != noErr) return fail(23);
    AudioDeviceID actual = kAudioObjectUnknown;
    bytes = sizeof(actual);
    if (api.getUnit(unit, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global,
        0, &actual, &bytes) != noErr || bytes != sizeof(actual) || actual != device) return fail(24);
    return YES;
}

/// 查询指定 CoreAudio 设备的显示名称，不可读时返回 nil。
static inline NSString *MSIMEVoiceCaptureDeviceName(
    AudioDeviceID device,
    const MSIMEVoiceCaptureDeviceListAPI &api = {}) {
    if (device == kAudioObjectUnknown || !api.get) return nil;
    AudioObjectPropertyAddress nameAddress = {
        kAudioObjectPropertyName,
        kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyElementMain
    };
    CFStringRef nameValue = nullptr;
    UInt32 nameBytes = sizeof(nameValue);
    if (api.get(device, &nameAddress, 0, nullptr, &nameBytes, &nameValue) != noErr ||
        nameBytes != sizeof(nameValue) || !nameValue) {
        return nil;
    }
    NSString *name = [(__bridge NSString *)nameValue copy];
    CFRelease(nameValue);
    return name.length ? name : nil;
}

/// 查询当前系统默认输入设备的名称与 UID；没有可用输入设备时返回 nil。
static inline NSDictionary *MSIMEVoiceDefaultInputDeviceDescriptor(
    const MSIMEVoiceCaptureDeviceListAPI &api = {}) {
    NSArray<NSDictionary *> *devices = MSIMEListVoiceCaptureDevices(api);
    if (!devices.count) return nil;
    // MSIMEListVoiceCaptureDevices 保证系统默认设备排在首位；若默认设备无输入流则回退至首个可用输入设备。
    return devices.firstObject;
}

/// 按首选 UID 解析一个当前存活的输入设备；若首选设备已断开，则回退到当前系统默认（或首个可用）输入设备。
/// 返回 NO 仅表示系统当前没有任何可用录音设备。
static inline BOOL MSIMEVoiceResolveCaptureDeviceWithFallback(
    NSString *preferredUID,
    NSString * __autoreleasing *resolvedUIDOut,
    NSString * __autoreleasing *resolvedNameOut,
    BOOL *usedFallbackOut,
    const MSIMEVoiceCaptureDeviceListAPI &api = {}) {
    if (resolvedUIDOut) *resolvedUIDOut = nil;
    if (resolvedNameOut) *resolvedNameOut = nil;
    if (usedFallbackOut) *usedFallbackOut = NO;
    NSArray<NSDictionary *> *devices = MSIMEListVoiceCaptureDevices(api);
    if (!devices.count) return NO;
    if (preferredUID.length) {
        for (NSDictionary *entry in devices) {
            if ([entry[@"uid"] isEqual:preferredUID]) {
                if (resolvedUIDOut) *resolvedUIDOut = entry[@"uid"];
                if (resolvedNameOut) *resolvedNameOut = entry[@"name"];
                return YES;
            }
        }
        NSDictionary *fallback = devices.firstObject;
        if (resolvedUIDOut) *resolvedUIDOut = fallback[@"uid"];
        if (resolvedNameOut) *resolvedNameOut = fallback[@"name"];
        if (usedFallbackOut) *usedFallbackOut = YES;
        return YES;
    }
    NSDictionary *defaultDevice = devices.firstObject;
    if (resolvedUIDOut) *resolvedUIDOut = defaultDevice[@"uid"];
    if (resolvedNameOut) *resolvedNameOut = defaultDevice[@"name"];
    return YES;
}

/// 当当前正在使用的输入设备发生无帧卡死时，寻找除 `excludingUID` 外的另一台存活输入设备（优先返回系统默认或内置麦）。
/// 若系统中只有一台输入设备，则返回 nil（调用方重建原设备）。
static inline NSString *MSIMEVoiceAlternativeCaptureDeviceUID(
    NSString *excludingUID,
    NSString * __autoreleasing *resolvedNameOut,
    const MSIMEVoiceCaptureDeviceListAPI &api = {}) {
    if (resolvedNameOut) *resolvedNameOut = nil;
    NSArray<NSDictionary *> *devices = MSIMEListVoiceCaptureDevices(api);
    if (devices.count < 2) return nil;
    NSDictionary *firstAlternative = nil;
    for (NSDictionary *entry in devices) {
        NSString *uid = entry[@"uid"];
        if (!uid.length || (excludingUID.length && [uid isEqual:excludingUID])) continue;
        if (!firstAlternative) firstAlternative = entry;
        // 优先选择内置麦克风作为蓝牙/外设卡死时的稳定退路。
        NSString *name = [entry[@"name"] isKindOfClass:NSString.class] ? [entry[@"name"] lowercaseString] : @"";
        if ([name containsString:@"built-in"] || [name containsString:@"macbook"] || [name containsString:@"内建"] || [name containsString:@"内置"]) {
            if (resolvedNameOut) *resolvedNameOut = entry[@"name"];
            return uid;
        }
    }
    if (firstAlternative) {
        if (resolvedNameOut) *resolvedNameOut = firstAlternative[@"name"];
        return firstAlternative[@"uid"];
    }
    return nil;
}

/// 配置录音设备并在所选外设已断开时自动回退至存活的系统默认输入设备。
/// 保留严格入口 `MSIMEConfigureVoiceCaptureDevice` 供单测与显式校验使用；本函数面向生产录音与热迁移。
static inline BOOL MSIMEConfigureVoiceCaptureDeviceWithFallback(
    NSString *preferredUID,
    AudioUnit unit,
    BOOL *usedFallbackOut,
    NSString * __autoreleasing *resolvedNameOut,
    NSError **error,
    const MSIMEVoiceCaptureDeviceAPI &api = {}) {
    if (usedFallbackOut) *usedFallbackOut = NO;
    if (resolvedNameOut) *resolvedNameOut = nil;
    MSIMEVoiceCaptureDeviceListAPI listAPI{api.get, api.size};
    if (!preferredUID.length) {
        NSDictionary *defaultDevice = MSIMEVoiceDefaultInputDeviceDescriptor(listAPI);
        if (resolvedNameOut) *resolvedNameOut = defaultDevice[@"name"];
        return YES;
    }
    NSError *directError = nil;
    if (MSIMEConfigureVoiceCaptureDevice(preferredUID, unit, &directError, api)) {
        for (NSDictionary *entry in MSIMEListVoiceCaptureDevices(listAPI)) {
            if ([entry[@"uid"] isEqual:preferredUID]) {
                if (resolvedNameOut) *resolvedNameOut = entry[@"name"];
                break;
            }
        }
        return YES;
    }
    NSString *fallbackUID = nil;
    NSString *fallbackName = nil;
    BOOL fellBack = NO;
    if (!MSIMEVoiceResolveCaptureDeviceWithFallback(preferredUID, &fallbackUID, &fallbackName, &fellBack, listAPI) ||
        !fallbackUID.length) {
        if (error) *error = directError;
        return NO;
    }
    // 尝试将音频单元显式切到存活回退设备；若回退设备本身即系统默认且无 unit（测试夹具）或切换成功则放行。
    if (!MSIMEConfigureVoiceCaptureDevice(fallbackUID, unit, &directError, api)) {
        if (error) *error = directError;
        return NO;
    }
    if (usedFallbackOut) *usedFallbackOut = YES;
    if (resolvedNameOut) *resolvedNameOut = fallbackName;
    return YES;
}
