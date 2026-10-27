#pragma once
#import "EditionIdentity.h"

// Shurufa 的常驻服务只从安装目录启动。身份查询和注册状态查询不创建服务。
static inline BOOL MSIMEHostCanServeIn(NSDictionary *info, NSString *bundlePath, NSString *home) {
    if (!MSIMEEditionIsShurufaIn(info)) return YES;
    NSString *user = [home stringByAppendingPathComponent:@"Library/Input Methods/Shurufa.app"];
    NSString *system = @"/Library/Input Methods/Shurufa.app";
    NSString *actual = [bundlePath stringByResolvingSymlinksInPath];
    return [actual isEqualToString:[user stringByResolvingSymlinksInPath]] ||
           [actual isEqualToString:[system stringByResolvingSymlinksInPath]];
}

static inline void MSIMEUnregisterUninstalledHost(void) {
    NSTask *task = [NSTask new];
    task.executableURL = [NSURL fileURLWithPath:@"/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"];
    task.arguments = @[@"-u", NSBundle.mainBundle.bundlePath];
    task.standardOutput = NSFileHandle.fileHandleWithNullDevice;
    task.standardError = NSFileHandle.fileHandleWithNullDevice;
    if ([task launchAndReturnError:nil]) [task waitUntilExit];
}
