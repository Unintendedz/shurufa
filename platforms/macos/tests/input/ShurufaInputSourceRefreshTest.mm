#import "../../src/input/InputSourceRegistration.h"
#include <cassert>
#include <cstdio>

static NSString *const Bundle = @"dev.shurufa.inputmethod.Shurufa";
static NSMutableArray<NSMutableDictionary *> *Registry;
static NSMutableArray<NSString *> *Trace;
static BOOL IgnoreDisable;
static NSString *IgnoreEnable;
static OSStatus RegistrationError;
static NSUInteger SelectionCalls;

extern "C" OSStatus TISSelectInputSource(TISInputSourceRef) {
    ++SelectionCalls;
    return noErr;
}

static NSMutableDictionary *Source(NSString *suffix, BOOL enabled) {
    NSString *identifier = suffix.length ? [Bundle stringByAppendingFormat:@".%@", suffix] : Bundle;
    return [@{(__bridge NSString *)kTISPropertyInputSourceID:identifier,
        (__bridge NSString *)kTISPropertyInputSourceIsEnabled:@(enabled),
        (__bridge NSString *)kTISPropertyInputSourceIsSelectCapable:@(suffix.length != 0)} mutableCopy];
}
static CFArrayRef List(CFDictionaryRef filter, Boolean all) {
    assert(all && [((__bridge NSDictionary *)filter)[(__bridge NSString *)kTISPropertyBundleID] isEqual:Bundle]);
    return (CFArrayRef)CFBridgingRetain([Registry copy]);
}
static void *Property(TISInputSourceRef source, CFStringRef key) {
    return (__bridge void *)((__bridge NSDictionary *)source)[(__bridge NSString *)key];
}
static OSStatus Register(CFURLRef location) {
    assert([(__bridge NSURL *)location isFileURL]);
    [Trace addObject:@"register"];
    if (RegistrationError) return RegistrationError;
    // 新登记对象与旧对象不同，且删除已退役的 Roman。
    Registry = [@[Source(@"", NO),Source(@"Hans",NO),Source(@"Shuangpin",NO)] mutableCopy];
    return noErr;
}
static OSStatus Disable(TISInputSourceRef source) {
    NSMutableDictionary *record = (__bridge NSMutableDictionary *)source;
    NSString *identifier = record[(__bridge NSString *)kTISPropertyInputSourceID];
    [Trace addObject:[@"disable:" stringByAppendingString:identifier]];
    if (!IgnoreDisable) {
        record[(__bridge NSString *)kTISPropertyInputSourceIsEnabled] = @NO;
    }
    return noErr;
}
static OSStatus Enable(TISInputSourceRef source) {
    NSMutableDictionary *record = (__bridge NSMutableDictionary *)source;
    NSString *identifier = record[(__bridge NSString *)kTISPropertyInputSourceID];
    [Trace addObject:[@"enable:" stringByAppendingString:identifier]];
    if (![identifier isEqual:IgnoreEnable]) record[(__bridge NSString *)kTISPropertyInputSourceIsEnabled] = @YES;
    return noErr;
}
static void Reset() {
    Registry = [@[Source(@"",YES),Source(@"Hans",YES),Source(@"Shuangpin",NO),Source(@"Roman",YES)] mutableCopy];
    Trace = [NSMutableArray array]; IgnoreDisable = NO; IgnoreEnable = nil; RegistrationError = noErr; SelectionCalls = 0;
}
static OSStatus Refresh(NSArray<NSString *> *previous) {
    return MSIMERefreshInstalledInputSources([NSURL fileURLWithPath:@"/synthetic/Input Methods/Shurufa.app"], Bundle,
        @[[Bundle stringByAppendingString:@".Hans"],[Bundle stringByAppendingString:@".Shuangpin"]],
        previous,Register,List,Property,Enable,Disable);
}
int main() {
    @autoreleasepool {
        const char *arguments[] = {"Shurufa","--refresh-installed-input-sources",""};
        assert(MSIMEShouldRefreshInstalledInputSources(3,arguments));
        assert(!MSIMEShouldRegisterInputSource(3,arguments));
        assert(!MSIMEShouldRefreshInstalledInputSources(1,arguments));
        NSString *hans = [Bundle stringByAppendingString:@".Hans"];
        NSString *shuangpin = [Bundle stringByAppendingString:@".Shuangpin"];
        Reset();
        assert(Refresh(@[hans,[Bundle stringByAppendingString:@".Roman"]]) == noErr);
        assert(([Trace isEqual:@[[@"disable:" stringByAppendingString:hans],[@"disable:" stringByAppendingFormat:@"%@.Roman",Bundle],
            [@"disable:" stringByAppendingString:Bundle],@"register",
            [@"enable:" stringByAppendingString:Bundle],[@"enable:" stringByAppendingString:hans]]]));
        assert(Registry.count == 3 && SelectionCalls == 0);
        Reset();
        assert(Refresh(@[shuangpin]) == noErr);
        assert(![Registry[1][(__bridge NSString *)kTISPropertyInputSourceIsEnabled] boolValue]);
        assert([Registry[2][(__bridge NSString *)kTISPropertyInputSourceIsEnabled] boolValue]);
        assert(SelectionCalls == 0);
        Reset(); Registry = [NSMutableArray array];
        assert(Refresh(@[]) == noErr && [Trace.firstObject isEqual:@"register"] && SelectionCalls == 0);
        Reset(); IgnoreDisable = YES;
        assert(Refresh(@[hans]) != noErr && ![Trace containsObject:@"register"] && SelectionCalls == 0);
        Reset(); IgnoreEnable = hans;
        assert(Refresh(@[hans]) != noErr && SelectionCalls == 0);
        Reset(); RegistrationError = -777;
        assert(Refresh(@[hans]) == -777 && Trace.count == 4 && SelectionCalls == 0);
        puts("Shurufa source refresh: remove/register/enable order, declared modes, readback and select=0 verified");
    }
}
