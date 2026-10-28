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
        NSDictionary *identity = @{@"MSIMEEdition":@"shurufa", @"CFBundleDisplayName":@"Shurufa"};
        assert([MSIMEEditionTextIn(identity, @"水杉输入法设置") isEqual:@"Shurufa设置"]);
        assert([MSIMEEditionTextIn(identity, @"水杉 IME / Metasequoia IME / MSIME") isEqual:@"Shurufa / Shurufa / Shurufa"]);
        assert([MSIMEEditionTextIn(@{}, @"水杉输入法设置") isEqual:@"水杉输入法设置"]);
        assert(!MSIMEEditionOffersUpstreamLinksIn(identity) && MSIMEEditionOffersUpstreamLinksIn(@{}));
    }
}
