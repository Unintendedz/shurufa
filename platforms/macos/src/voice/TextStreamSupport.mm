#import "TextStreamSupport.h"
#include <sys/stat.h>

NSString *MSIMETextStreamRootDirectory(void) {
    NSString *configured = NSProcessInfo.processInfo.environment[@"SHURUFA_TEXTSTREAM_ROOT"];
    if (configured.length && configured.isAbsolutePath) return configured;
    return [NSHomeDirectory() stringByAppendingPathComponent:@"Library/Application Support/Shurufa/textstream"];
}

NSString *MSIMETextStreamModelDirectory(void) {
    return [MSIMETextStreamRootDirectory() stringByAppendingPathComponent:@"model"];
}

BOOL MSIMETextStreamReady(void) {
    struct stat info = {};
    NSArray<NSString *> *files = @[
        [MSIMETextStreamRootDirectory() stringByAppendingPathComponent:@".ready"],
        [MSIMETextStreamModelDirectory() stringByAppendingPathComponent:@"config.json"],
        [MSIMETextStreamModelDirectory() stringByAppendingPathComponent:@"model.safetensors"],
    ];
    for (NSString *file in files)
        if (stat(file.fileSystemRepresentation, &info) != 0 || !S_ISREG(info.st_mode)) return NO;
    return YES;
}

static NSString *TextStreamResource(NSString *name, NSString *environmentKey) {
    NSFileManager *files = NSFileManager.defaultManager;
    NSString *configured = NSProcessInfo.processInfo.environment[environmentKey];
    if (configured.length) return configured.isAbsolutePath && [files isExecutableFileAtPath:configured] ? configured : nil;
    NSString *bundled = [[NSBundle.mainBundle.bundlePath stringByAppendingPathComponent:@"Contents/Resources/textstream"] stringByAppendingPathComponent:name];
    return [files isExecutableFileAtPath:bundled] ? bundled : nil;
}

NSString *MSIMETextStreamLauncherPath(void) {
    return TextStreamResource(@"shurufa-textstream", @"MSIME_TEXTSTREAM_LAUNCHER");
}

NSString *MSIMETextStreamInstallerPath(void) {
    NSString *launcher = MSIMETextStreamLauncherPath();
    NSString *installer = [launcher.stringByDeletingLastPathComponent stringByAppendingPathComponent:@"install-runtime.sh"];
    return launcher && [NSFileManager.defaultManager isExecutableFileAtPath:installer] ? installer : nil;
}

namespace {
NSObject *InstallLock() {
    static NSObject *lock;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ lock = [NSObject new]; });
    return lock;
}
BOOL installing = NO;
}

BOOL MSIMETextStreamInstalling(void) {
    @synchronized(InstallLock()) { return installing; }
}

void MSIMETextStreamInstall(void (^completion)(BOOL, NSString *)) {
    void (^finish)(BOOL, NSString *) = ^(BOOL success, NSString *summary) {
        dispatch_async(dispatch_get_main_queue(), ^{ if (completion) completion(success, summary); });
    };
    NSString *installer = MSIMETextStreamInstallerPath();
    if (!installer) { finish(NO, @"找不到随包的安装脚本，请重新安装输入法。"); return; }
    @synchronized(InstallLock()) {
        if (installing) { finish(NO, @"语音模型正在安装中。"); return; }
        installing = YES;
    }
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
        NSTask *task = [NSTask new];
        task.executableURL = [NSURL fileURLWithPath:@"/bin/bash"];
        task.arguments = @[ installer ];
        NSMutableDictionary *environment = [NSProcessInfo.processInfo.environment mutableCopy];
        // 输入法进程的 PATH 只有系统目录，uv 通常装在 Homebrew 或 ~/.local/bin。
        NSString *extra = [NSString stringWithFormat:@"/opt/homebrew/bin:/usr/local/bin:%@/.local/bin", NSHomeDirectory()];
        environment[@"PATH"] = [NSString stringWithFormat:@"%@:%@", extra, environment[@"PATH"] ?: @"/usr/bin:/bin"];
        task.environment = environment;
        NSPipe *output = [NSPipe pipe];
        task.standardOutput = output;
        task.standardError = output;
        task.standardInput = NSFileHandle.fileHandleWithNullDevice;
        NSError *launchError = nil;
        BOOL success = NO;
        NSString *summary = nil;
        if (![task launchAndReturnError:&launchError]) {
            summary = @"无法启动安装脚本。";
        } else {
            NSData *data = [output.fileHandleForReading readDataToEndOfFile];
            [task waitUntilExit];
            NSString *text = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding] ?: @"";
            NSString *last = @"";
            for (NSString *line in [text componentsSeparatedByCharactersInSet:NSCharacterSet.newlineCharacterSet])
                if (line.length) last = line;
            if (last.length > 300) last = [last substringFromIndex:last.length - 300];
            success = task.terminationStatus == 0 && MSIMETextStreamReady();
            summary = success ? @"语音模型已安装。" : [NSString stringWithFormat:@"安装失败：%@", last.length ? last : @"没有输出"];
        }
        @synchronized(InstallLock()) { installing = NO; }
        finish(success, summary);
    });
}
