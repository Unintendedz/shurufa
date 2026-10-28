#pragma once
#import <AppKit/AppKit.h>

// 每个键独立布局；文字键由固有宽度和左右内边距决定，左右侧别只作小字标记。
static inline NSView *MSIMEKeycap(NSString *symbol, NSString *side = nil) {
    NSMutableAttributedString *title = [NSMutableAttributedString new];
    if (side.length) [title appendAttributedString:[[NSAttributedString alloc] initWithString:[side stringByAppendingString:@" "]
        attributes:@{NSFontAttributeName:[NSFont systemFontOfSize:9],NSForegroundColorAttributeName:NSColor.secondaryLabelColor}]];
    [title appendAttributedString:[[NSAttributedString alloc] initWithString:symbol
        attributes:@{NSFontAttributeName:[NSFont systemFontOfSize:14 weight:NSFontWeightMedium],NSForegroundColorAttributeName:NSColor.labelColor}]];
    NSTextField *label = [NSTextField labelWithAttributedString:title];
    label.translatesAutoresizingMaskIntoConstraints = NO;
    NSBox *key = [NSBox new];
    key.boxType = NSBoxCustom; key.titlePosition = NSNoTitle;
    key.borderWidth = 0.5; key.cornerRadius = 5;
    key.borderColor = NSColor.separatorColor; key.fillColor = NSColor.controlBackgroundColor;
    key.contentViewMargins = NSZeroSize; key.translatesAutoresizingMaskIntoConstraints = NO;
    [key.contentView addSubview:label];
    [NSLayoutConstraint activateConstraints:@[
        [label.leadingAnchor constraintEqualToAnchor:key.contentView.leadingAnchor constant:8],
        [label.trailingAnchor constraintEqualToAnchor:key.contentView.trailingAnchor constant:-8],
        [label.topAnchor constraintEqualToAnchor:key.contentView.topAnchor constant:3],
        [label.bottomAnchor constraintEqualToAnchor:key.contentView.bottomAnchor constant:-3],
        [key.widthAnchor constraintGreaterThanOrEqualToConstant:28]
    ]];
    return key;
}

static inline NSStackView *MSIMEKeycapRow(NSArray<NSView *> *keys, NSString *accessibleTitle) {
    NSStackView *row = [NSStackView stackViewWithViews:keys];
    row.orientation = NSUserInterfaceLayoutOrientationHorizontal;
    row.alignment = NSLayoutAttributeCenterY; row.spacing = 4;
    row.accessibilityLabel = accessibleTitle;
    return row;
}

// 这些行对应宿主的固定切换开关，绑定由原控制器读取；只替换显示，不另造配置。
static inline NSView *MSIMEKeycapSettingLabel(NSString *title) {
    NSDictionary *entries = @{
        @"Shift + 空格切换中英文":@[@"切换中英文",@"⇧",@"空格"],
        @"单按 Shift 切换中英文":@[@"单按切换中英文",@"⇧"],
        @"单按 Control 切换中英文":@[@"单按切换中英文",@"⌃"],
        @"单按 Caps Lock 切换中英文":@[@"轻点切换中英文",@"⇪"],
        @"Control + Option + 空格切换中英文":@[@"切换中英文",@"⌃",@"⌥",@"空格"],
        @"Control + Shift + F 切换简繁":@[@"切换简繁",@"⌃",@"⇧",@"F"],
        @"Option + Shift + H 切换全半角":@[@"切换全半角",@"⌥",@"⇧",@"H"]
    };
    NSArray *entry = entries[title];
    if (!entry) return nil;
    NSMutableArray<NSView *> *items = [NSMutableArray array];
    for (NSUInteger i=1; i<entry.count; ++i) [items addObject:MSIMEKeycap(entry[i])];
    NSTextField *label = [NSTextField labelWithString:entry[0]];
    label.font = [NSFont systemFontOfSize:13];
    [items addObject:label];
    return MSIMEKeycapRow(items, title);
}
