#import "../../src/settings/ShurufaPreferenceMigration.h"
#include <cassert>

int main() {
    @autoreleasepool {
        NSString *suite = [@"shurufa.synthetic.migration." stringByAppendingString:NSUUID.UUID.UUIDString];
        NSUserDefaults *defaults = [[NSUserDefaults alloc] initWithSuiteName:suite];
        __block NSDictionary *document = @{@"format_version":@1, @"revision":@7,
            @"preferences":@{@"scheme":@"shuangpin", @"voice_input":@{@"enabled":@YES},
                @"keybindings":@{@"switch_language_caps_lock":@NO, @"switch_language_shift":@NO}}};
        __block NSUInteger saves = 0;
        auto load = ^NSDictionary *{ return document; };
        auto save = ^NSDictionary *(uint64_t revision, NSDictionary *value) {
            assert(revision == 7);
            ++saves;
            document = value;
            return value;
        };
        // 保存失败时不标记完成，原有偏好和本地镜像都不改。
        assert(!MSIMEMigrateShurufaCapsLock(defaults, load, ^NSDictionary *(uint64_t, NSDictionary *) { return nil; }));
        assert(![defaults boolForKey:@"ShurufaCapsLockMigrationV1"]);
        assert(MSIMEMigrateShurufaCapsLock(defaults, load, save));
        assert(saves == 1 && [defaults boolForKey:@"MSIMEClientCapsLockTapShortcut"]);
        NSDictionary *preferences = document[@"preferences"];
        assert([preferences[@"scheme"] isEqual:@"shuangpin"]);
        assert([preferences[@"voice_input"] isEqual:@{@"enabled":@YES}]);
        assert([preferences[@"keybindings"][@"switch_language_caps_lock"] isEqual:@YES]);
        assert([preferences[@"keybindings"][@"switch_language_shift"] isEqual:@NO]);
        // 用户之后关闭，重复安装不能再次覆盖。
        [defaults setBool:NO forKey:@"MSIMEClientCapsLockTapShortcut"];
        assert(MSIMEMigrateShurufaCapsLock(defaults, load, save));
        assert(saves == 1 && ![defaults boolForKey:@"MSIMEClientCapsLockTapShortcut"]);
        [defaults removePersistentDomainForName:suite];
    }
}
