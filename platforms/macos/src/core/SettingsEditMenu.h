#pragma once
#import <AppKit/AppKit.h>
#import <os/log.h>

// 输入法没有默认主菜单；文本框的 Command 快捷键仍需要标准编辑菜单分发到响应链。
static inline void MSIMEInstallSettingsEditMenu(void) {
    NSMenu *main = NSApp.mainMenu;
    for (NSMenuItem *item in main.itemArray)
        if ([item.identifier isEqual:@"ShurufaStandardEditMenu"]) return;
    if (!main) main = [[NSMenu alloc] initWithTitle:@""];
    NSMenuItem *editItem = [[NSMenuItem alloc] initWithTitle:@"编辑" action:nil keyEquivalent:@""];
    editItem.identifier = @"ShurufaStandardEditMenu";
    NSMenu *edit = [[NSMenu alloc] initWithTitle:@"编辑"];
    for (NSArray<NSString *> *entry in @[@[@"撤销", @"undo:", @"z"], @[@"重做", @"redo:", @"Z"],
        @[@"剪切", @"cut:", @"x"], @[@"复制", @"copy:", @"c"], @[@"粘贴", @"paste:", @"v"],
        @[@"全选", @"selectAll:", @"a"]]) {
        NSMenuItem *item = [[NSMenuItem alloc] initWithTitle:entry[0] action:NSSelectorFromString(entry[1]) keyEquivalent:entry[2]];
        item.target = nil;
        item.keyEquivalentModifierMask = NSEventModifierFlagCommand | ([entry[1] isEqual:@"redo:"] ? NSEventModifierFlagShift : 0);
        [edit addItem:item];
        if ([entry[1] isEqual:@"redo:"]) [edit addItem:NSMenuItem.separatorItem];
    }
    editItem.submenu = edit;
    [main addItem:editItem];
    NSApp.mainMenu = main;
    // 诊断只记录菜单状态和进程策略，不读取文本框或剪贴板内容。
    os_log(OS_LOG_DEFAULT, "settings_edit_menu installed=1 commands=%lu activation_policy=%ld",
        (unsigned long)edit.numberOfItems, (long)NSApp.activationPolicy);
}
