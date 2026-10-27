#import "../../src/core/InstalledHost.h"
#include <cassert>

int main() {
    @autoreleasepool {
        NSDictionary *shurufa = @{@"MSIMEEdition": @"shurufa"};
        NSString *home = @"/synthetic/home";
        assert(MSIMEHostCanServeIn(shurufa, @"/synthetic/home/Library/Input Methods/Shurufa.app", home));
        assert(MSIMEHostCanServeIn(shurufa, @"/Library/Input Methods/Shurufa.app", home));
        assert(!MSIMEHostCanServeIn(shurufa, @"/Library/Input Methods/Other.app", home));
        assert(!MSIMEHostCanServeIn(shurufa, @"/synthetic/build/Shurufa.app", home));
        assert(!MSIMEHostCanServeIn(shurufa, @"/synthetic/home/Library/Application Support/Shurufa/backup/Shurufa.app", home));
        assert(MSIMEHostCanServeIn(@{}, @"/synthetic/build/水杉输入法.app", home));
    }
}
