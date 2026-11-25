#import "TextClient.h"

#if TARGET_OS_OSX
// NSUnderlineStyle* and the marked clause attribute are AppKit's, not Foundation's.
#import <AppKit/AppKit.h>
#endif

static BOOL MSIMEPreeditSeparator(unichar character) {
    return character == '\'' || character == ' ';
}

static BOOL MSIMEStrictCaretPosition(id value, NSUInteger *result) {
    if (![value isKindOfClass:NSNumber.class] ||
        CFGetTypeID((__bridge CFTypeRef)value) == CFBooleanGetTypeID() ||
        CFNumberIsFloatType((__bridge CFNumberRef)value)) return NO;
    NSNumber *number = (NSNumber *)value;
    if ([number compare:@0] == NSOrderedAscending) return NO;
    uint64_t position = number.unsignedLongLongValue;
    if ([number compare:@(position)] != NSOrderedSame || position > NSUIntegerMax) return NO;
    if (result) *result = (NSUInteger)position;
    return YES;
}

// 把按 Unicode 标量计的位置换成 UTF-16 下标；不是非负整数或超出范围时取末尾。
static NSUInteger MSIMEScalarOffsetFromCount(NSString *text, NSUInteger scalars, BOOL valid) {
    if (!valid) return text.length;
    NSUInteger offset = 0;
    while (offset < text.length && scalars > 0) {
        const unichar high = [text characterAtIndex:offset];
        offset += (high >= 0xD800 && high <= 0xDBFF && offset + 1 < text.length) ? 2 : 1;
        --scalars;
    }
    return offset;
}

static NSString *MSIMEPreeditLetters(NSString *text) {
    NSMutableString *letters = [NSMutableString string];
    for (NSUInteger i = 0; i < text.length; ++i) {
        unichar character = [text characterAtIndex:i];
        if (MSIMEPreeditSeparator(character)) continue;
        if (!((character >= 'a' && character <= 'z') || (character >= 'A' && character <= 'Z'))) return nil;
        [letters appendString:[text substringWithRange:NSMakeRange(i, 1)]];
    }
    return letters;
}

static NSUInteger MSIMEPreeditCaretPositionFromValue(NSString *editing, NSString *preedit,
                                                       NSUInteger rawCaret, BOOL validCaret) {
    if (!validCaret) return preedit.length;
    rawCaret = MIN(rawCaret, editing.length);
    if ([preedit isEqual:editing]) return rawCaret;
    // Only map lossless separator formatting. Expanded shuangpin, converted words
    // and corrections need an Engine-provided offset map, not host-side guesses.
    NSString *letters = MSIMEPreeditLetters(editing);
    if (!letters.length || ![letters isEqual:MSIMEPreeditLetters(preedit)]) return preedit.length;
    if (rawCaret == editing.length) return preedit.length;
    NSUInteger remaining = 0;
    for (NSUInteger i = 0; i < rawCaret; ++i)
        if (!MSIMEPreeditSeparator([editing characterAtIndex:i])) ++remaining;
    NSUInteger displayCaret = 0;
    while (displayCaret < preedit.length && remaining) {
        if (!MSIMEPreeditSeparator([preedit characterAtIndex:displayCaret])) --remaining;
        ++displayCaret;
    }
    // Preserve the two sides of an explicitly typed syllable separator, matching
    // Windows GetPreeditWithCaretMarker at the pinned product reference.
    if (rawCaret && MSIMEPreeditSeparator([editing characterAtIndex:rawCaret - 1]))
        while (displayCaret < preedit.length && MSIMEPreeditSeparator([preedit characterAtIndex:displayCaret])) ++displayCaret;
    return displayCaret;
}

NSUInteger MSIMEPreeditCaretPosition(NSString *editing, NSString *preedit, id position) {
    NSUInteger rawCaret = 0;
    const BOOL validCaret = MSIMEStrictCaretPosition(position, &rawCaret);
    return MSIMEPreeditCaretPositionFromValue(editing, preedit, rawCaret, validCaret);
}

NSString *MSIMETextClientFollowingCharacter(id<MSIMETextClient> client) {
    if (!client || ![client respondsToSelector:@selector(selectedRange)] ||
        ![client respondsToSelector:@selector(attributedSubstringFromRange:)]) return nil;
    NSRange selected = [client selectedRange];
    if (selected.location == NSNotFound || selected.length != 0) return nil;
    NSAttributedString *substring = [client attributedSubstringFromRange:NSMakeRange(selected.location, 1)];
    NSString *text = substring.string;
    return text.length == 1 ? [text substringWithRange:NSMakeRange(0, 1)] : nil;
}

// 客户端可能把截断的代理对扩展为两个 UTF-16 单元；除此以外的长度不猜测位置。
static uint32_t MSIMETextClientScalarBefore(id<MSIMETextClient> client, NSUInteger location, BOOL (^isCurrent)(void)) {
    if (!location || location == NSNotFound) return 0;
    NSString *text = [client attributedSubstringFromRange:NSMakeRange(location - 1, 1)].string;
    if (isCurrent && !isCurrent()) return 0;
    if (text.length == 2) {
        const unichar high = [text characterAtIndex:0], low = [text characterAtIndex:1];
        return CFStringIsSurrogateHighCharacter(high) && CFStringIsSurrogateLowCharacter(low)
            ? CFStringGetLongCharacterForSurrogatePair(high, low) : 0;
    }
    if (text.length != 1) return 0;
    const unichar tail = [text characterAtIndex:0];
    if (CFStringIsSurrogateLowCharacter(tail) && location > 1) {
        NSString *pair = [client attributedSubstringFromRange:NSMakeRange(location - 2, 2)].string;
        if (isCurrent && !isCurrent()) return 0;
        if (pair.length == 2 && CFStringIsSurrogateHighCharacter([pair characterAtIndex:0]) &&
            [pair characterAtIndex:1] == tail)
            return CFStringGetLongCharacterForSurrogatePair([pair characterAtIndex:0], tail);
        return 0;
    }
    return CFStringIsSurrogateHighCharacter(tail) || CFStringIsSurrogateLowCharacter(tail) ? 0 : tail;
}

uint32_t MSIMETextClientPrecedingUnicodeScalar(id<MSIMETextClient> client) {
    if (!client || ![client respondsToSelector:@selector(selectedRange)] ||
        ![client respondsToSelector:@selector(attributedSubstringFromRange:)]) return 0;
    NSRange selected = [client selectedRange];
    return selected.length == 0 ? MSIMETextClientScalarBefore(client, selected.location, nil) : 0;
}

NSString *MSIMETextClientPrecedingContext(id<MSIMETextClient> client, NSUInteger maxUTF16Units,
                                          BOOL knownReadable, BOOL *provedReadable) {
    if (provedReadable) *provedReadable = NO;
    if (!client || !maxUTF16Units || ![client respondsToSelector:@selector(selectedRange)] ||
        ![client respondsToSelector:@selector(attributedSubstringFromRange:)])
        return nil;
    const NSRange selected = [client selectedRange];
    if (selected.location == NSNotFound) return nil;
    if (selected.location == 0) {
        NSAttributedString *after = [client attributedSubstringFromRange:NSMakeRange(0, 1)];
        if (after.string.length > 0) {
            if (provedReadable) *provedReadable = YES;
            return @"";
        }
        return knownReadable ? @"" : nil;
    }
    const NSUInteger start = selected.location > maxUTF16Units ? selected.location - maxUTF16Units : 0;
    const NSUInteger length = selected.location - start;
    NSAttributedString *substring = [client attributedSubstringFromRange:NSMakeRange(start, length)];
    NSString *text = substring.string;
    // 光标不在文档起点（length >= 1）却返回空串说明客户端未真正暴露前文（如部分 Chromium/Electron 输入框），
    // 必须返回 nil 让上层回退到 AX 读取，而不能当作已知空文档覆盖运行时已有前文。
    if (!text || !text.length) return nil;
    if (provedReadable) *provedReadable = YES;
    NSUInteger trimStart = 0;
    if (CFStringIsSurrogateLowCharacter([text characterAtIndex:0])) trimStart = 1;
    NSUInteger trimEnd = text.length;
    if (trimEnd > trimStart && CFStringIsSurrogateHighCharacter([text characterAtIndex:trimEnd - 1]))
        --trimEnd;
    if (trimStart >= trimEnd) return @"";
    if (trimStart == 0 && trimEnd == text.length) return text;
    return [text substringWithRange:NSMakeRange(trimStart, trimEnd - trimStart)];
}

NSString *MSIMETextClientFollowingContext(id<MSIMETextClient> client, NSUInteger maxUTF16Units) {
    if (!client || !maxUTF16Units || ![client respondsToSelector:@selector(selectedRange)] ||
        ![client respondsToSelector:@selector(attributedSubstringFromRange:)])
        return nil;
    @try {
        const NSRange selected = [client selectedRange];
        if (selected.location == NSNotFound) return nil;
        const NSUInteger afterStart = selected.location + selected.length;
        NSUInteger reqLen = maxUTF16Units;
        if ([client respondsToSelector:@selector(length)]) {
            const NSInteger totalLen = [client length];
            if (totalLen >= 0) {
                const NSUInteger docLen = static_cast<NSUInteger>(totalLen);
                if (afterStart >= docLen) return @"";
                reqLen = MIN(reqLen, docLen - afterStart);
            }
        }
        NSAttributedString *substring = [client attributedSubstringFromRange:NSMakeRange(afterStart, reqLen)];
        if (!substring && reqLen > 1) {
            // 兼容不自动裁剪 proposedRange 的客户端：先探 1 码元确认是否真有后文，再二分定位可读尾部长度。
            NSAttributedString *probe1 = [client attributedSubstringFromRange:NSMakeRange(afterStart, 1)];
            if (probe1.string.length > 0) {
                NSUInteger lo = 1;
                NSUInteger hi = reqLen - 1;
                NSAttributedString *best = probe1;
                while (lo <= hi) {
                    const NSUInteger mid = lo + (hi - lo) / 2;
                    NSAttributedString *cand = [client attributedSubstringFromRange:NSMakeRange(afterStart, mid)];
                    if (cand.string.length > 0) {
                        best = cand;
                        lo = mid + 1;
                    } else {
                        if (mid == 0) break;
                        hi = mid - 1;
                    }
                }
                substring = best;
            }
        }
        NSString *text = substring.string;
        if (!text) return nil;
        if (!text.length) return @"";
        NSUInteger trimStart = 0;
        if (CFStringIsSurrogateLowCharacter([text characterAtIndex:0])) trimStart = 1;
        NSUInteger trimEnd = text.length;
        if (trimEnd > trimStart && CFStringIsSurrogateHighCharacter([text characterAtIndex:trimEnd - 1]))
            --trimEnd;
        if (trimStart >= trimEnd) return @"";
        if (trimStart == 0 && trimEnd == text.length) return text;
        return [text substringWithRange:NSMakeRange(trimStart, trimEnd - trimStart)];
    } @catch (NSException *) {
        return nil;
    }
}

void MSIMEApplyTransitionWithWriteGuard(NSDictionary *transition, id<MSIMETextClient> client,
                                       MSIMEInlinePreeditStyle style, NSString *closing,
                                       BOOL *clientHasMarkedText, BOOL (^isCurrent)(void)) {
    MSIMEApplyTransitionWithCommitObserver(transition, client, style, closing,
                                           clientHasMarkedText, isCurrent, nil);
}

void MSIMEApplyTransitionWithPreeditStyle(NSDictionary *transition, id<MSIMETextClient> client,
                                          MSIMEInlinePreeditStyle style) {
    MSIMEApplyTransitionWithPendingClosing(transition, client, style, nil);
}

void MSIMEApplyTransitionWithPendingClosing(NSDictionary *transition, id<MSIMETextClient> client,
                                            MSIMEInlinePreeditStyle style, NSString *closing) {
    MSIMEApplyTransitionTrackingMarkedText(transition, client, style, closing, NULL);
}

void MSIMEApplyTransitionTrackingMarkedText(NSDictionary *transition, id<MSIMETextClient> client,
                                            MSIMEInlinePreeditStyle style, NSString *closing,
                                            BOOL *clientHasMarkedText) {
    MSIMEApplyTransitionWithCommitObserver(transition, client, style, closing, clientHasMarkedText, nil, nil);
}

static NSString *MSIMESliceString(ShurufaStringSlice slice) {
    if (!slice.ptr || slice.len == 0) return @"";
    return [[NSString alloc] initWithBytes:slice.ptr length:slice.len encoding:NSUTF8StringEncoding] ?: @"";
}

static void MSIMEApplyTransitionFields(NSString *commit, NSString *editing, NSString *preedit,
                                       NSUInteger rawCaret, BOOL validCaret, NSString *reading,
                                       NSString *conversion, NSUInteger conversionFocusStart,
                                       NSUInteger conversionFocusEnd, NSString *phrase,
                                       BOOL hasView, id<MSIMETextClient> client,
                                       MSIMEInlinePreeditStyle style, NSString *closing,
                                       BOOL *clientHasMarkedText, BOOL (^isCurrent)(void),
                                       void (^didCommit)(NSString *text)) {
    if (isCurrent && !isCurrent()) return;

    if (!closing.length) closing = nil;
    // A commit ends the pair: the closing mark goes in with the text it was holding open, and the
    // caret lands after it, where the next character belongs.
    if (commit && closing) commit = [commit stringByAppendingString:closing];
    const BOOL hasCommit = commit != nil;
    if (hasCommit) {
        // 先消耗旧标记，再同步写入；回调中的新组字由新的派发持有。
        if (clientHasMarkedText) *clientHasMarkedText = NO;
        [client insertText:commit replacementRange:NSMakeRange(NSNotFound, NSNotFound)];
        if (isCurrent && !isCurrent()) return;
        if (didCommit) didCommit(commit);
        if (isCurrent && !isCurrent()) return;
    }
    if (!hasView) return;
    if (!editing) editing = @"";
    if (!preedit) preedit = editing;
    if (reading.length && (!validCaret || rawCaret >= editing.length)) {
        editing = reading;
        preedit = reading;
        rawCaret = reading.length;
        validCaret = YES;
    }
    NSString *marked = preedit;
    NSUInteger caret = MSIMEPreeditCaretPositionFromValue(editing, preedit, rawCaret, validCaret);
    // 整句改字：行内画改好的整句，不管预编辑样式选的是什么，光标在焦点字前，焦点那一段单独成一个子句。
    NSRange focus = NSMakeRange(NSNotFound, 0);
    if (conversion.length) {
        const NSUInteger start = MSIMEScalarOffsetFromCount(conversion, conversionFocusStart, YES);
        const NSUInteger end = MAX(start, MSIMEScalarOffsetFromCount(conversion, conversionFocusEnd, YES));
        marked = conversion;
        caret = start;
        focus = NSMakeRange(start, end - start);
    } else if (style == MSIMEInlinePreeditStyleRaw) {
        marked = editing;
        caret = validCaret ? MIN(rawCaret, editing.length) : editing.length;
    } else if (style == MSIMEInlinePreeditStyleEmpty) {
        marked = @"";
        caret = 0;
    }
    NSUInteger phraseLength = 0;
    if (phrase.length && (style != MSIMEInlinePreeditStyleEmpty || focus.location != NSNotFound)) {
        marked = [phrase stringByAppendingString:marked];
        caret += phrase.length;
        phraseLength = phrase.length;
        if (focus.location != NSNotFound) focus.location += phrase.length;
    }
    // While the pair is open the closing mark is the tail of the marked text, so it stays visible and
    // stays after the caret. A commit above has already consumed it.
    if (closing && !hasCommit) marked = [marked stringByAppendingString:closing];
    id displayed = marked;
#if TARGET_OS_OSX
    if (focus.location != NSNotFound && focus.length > 0 && NSMaxRange(focus) <= marked.length) {
        NSMutableAttributedString *clauses = [[NSMutableAttributedString alloc] initWithString:marked];
        NSInteger segment = 0;
        if (focus.location > 0)
            [clauses addAttributes:@{NSUnderlineStyleAttributeName: @(NSUnderlineStyleSingle),
                                     NSMarkedClauseSegmentAttributeName: @(segment++)}
                             range:NSMakeRange(0, focus.location)];
        [clauses addAttributes:@{NSUnderlineStyleAttributeName: @(NSUnderlineStyleThick),
                                 NSMarkedClauseSegmentAttributeName: @(segment++)}
                         range:focus];
        if (NSMaxRange(focus) < marked.length)
            [clauses addAttributes:@{NSUnderlineStyleAttributeName: @(NSUnderlineStyleSingle),
                                     NSMarkedClauseSegmentAttributeName: @(segment)}
                             range:NSMakeRange(NSMaxRange(focus), marked.length - NSMaxRange(focus))];
        displayed = clauses;
    } else if (phraseLength > 0 && phraseLength < marked.length) {
        NSMutableAttributedString *clauses = [[NSMutableAttributedString alloc] initWithString:marked];
        [clauses addAttributes:@{NSUnderlineStyleAttributeName: @(NSUnderlineStyleSingle),
                                 NSMarkedClauseSegmentAttributeName: @0}
                         range:NSMakeRange(0, phraseLength)];
        [clauses addAttributes:@{NSUnderlineStyleAttributeName: @(NSUnderlineStyleThick),
                                 NSMarkedClauseSegmentAttributeName: @1}
                         range:NSMakeRange(phraseLength, marked.length - phraseLength)];
        displayed = clauses;
    } else if (marked.length > 0) {
        // 普通组字也明确标出子句与下划线，不依赖 IMK 与客户端的默认标记样式。
        displayed = [[NSAttributedString alloc] initWithString:marked attributes:@{
            NSUnderlineStyleAttributeName: @(NSUnderlineStyleSingle),
            NSMarkedClauseSegmentAttributeName: @0
        }];
    }
#endif
    // Only the clear of a composition that is not there is skipped; after a commit the clear still goes out, as it always has.
    if (clientHasMarkedText && !*clientHasMarkedText && !marked.length && !hasCommit) return;
    // Recorded before the write: IMK can service the next key inside it, and that nested write lands in the client after this one, so it must also be the one whose state is left recorded.
    if (clientHasMarkedText) *clientHasMarkedText = marked.length > 0;
    [client setMarkedText:displayed selectionRange:NSMakeRange(caret, 0) replacementRange:NSMakeRange(NSNotFound, NSNotFound)];
}

void MSIMEApplyTransitionWithCommitObserver(NSDictionary *transition, id<MSIMETextClient> client,
                                            MSIMEInlinePreeditStyle style, NSString *closing,
                                            BOOL *clientHasMarkedText, BOOL (^isCurrent)(void),
                                            void (^didCommit)(NSString *text)) {
    if (![transition isKindOfClass:NSDictionary.class]) return;
    id rawCommit = transition[@"commit"];
    NSString *commit = [rawCommit isKindOfClass:NSString.class] ? (NSString *)rawCommit : nil;
    NSDictionary *view = transition[@"view"];
    const BOOL hasView = [view isKindOfClass:NSDictionary.class];
    if (!hasView) {
        MSIMEApplyTransitionFields(commit, nil, nil, 0, NO, nil, nil, 0, 0, nil, NO, client,
                                   style, closing, clientHasMarkedText, isCurrent, didCommit);
        return;
    }
    NSString *editing = [view[@"editing_text"] isKindOfClass:NSString.class] ? view[@"editing_text"] : @"";
    NSString *preedit = [view[@"preedit"] isKindOfClass:NSString.class] ? view[@"preedit"] : editing;
    NSUInteger rawCaret = 0;
    const BOOL validCaret = MSIMEStrictCaretPosition(view[@"caret_position"], &rawCaret);
    NSString *reading = [view[@"reading"] isKindOfClass:NSString.class] ? view[@"reading"] : nil;
    NSString *conversion = [view[@"conversion"] isKindOfClass:NSString.class] ? view[@"conversion"] : nil;
    NSUInteger focusStart = 0;
    NSUInteger focusEnd = 0;
    if (conversion.length) {
        NSUInteger s = 0, e = 0;
        const BOOL vs = MSIMEStrictCaretPosition(view[@"conversion_focus_start"], &s);
        const BOOL ve = MSIMEStrictCaretPosition(view[@"conversion_focus_end"], &e);
        focusStart = vs ? s : conversion.length;
        focusEnd = ve ? MAX(focusStart, e) : conversion.length;
    }
    NSString *phrase = [view[@"phrase_prefix"] isKindOfClass:NSString.class] ? view[@"phrase_prefix"] : nil;
    MSIMEApplyTransitionFields(commit, editing, preedit, rawCaret, validCaret, reading, conversion,
                               focusStart, focusEnd, phrase, YES, client, style, closing,
                               clientHasMarkedText, isCurrent, didCommit);
}

void MSIMEApplyTransitionSnapshotWithCommitObserver(const ShurufaTransitionSnapshot *transition,
                                                    NSString *overrideCommit,
                                                    id<MSIMETextClient> client,
                                                    MSIMEInlinePreeditStyle style, NSString *closing,
                                                    BOOL *clientHasMarkedText, BOOL (^isCurrent)(void),
                                                    void (^didCommit)(NSString *text)) {
    if (!transition) return;
    NSString *commit = overrideCommit ?: (transition->has_commit ? MSIMESliceString(transition->commit) : nil);
    const ShurufaViewSnapshot *view = &transition->view;
    NSString *editing = MSIMESliceString(view->editing_text);
    NSString *preedit = (view->preedit.ptr || view->preedit.len > 0) ? MSIMESliceString(view->preedit) : editing;
    NSString *reading = view->reading.len > 0 ? MSIMESliceString(view->reading) : nil;
    NSString *conversion = view->conversion.len > 0 ? MSIMESliceString(view->conversion) : nil;
    NSString *phrase = view->phrase_prefix.len > 0 ? MSIMESliceString(view->phrase_prefix) : nil;
    MSIMEApplyTransitionFields(commit, editing, preedit, view->caret_position, YES, reading, conversion,
                               view->conversion_focus_start, view->conversion_focus_end, phrase,
                               YES, client, style, closing, clientHasMarkedText, isCurrent, didCommit);
}

void MSIMEApplyTransition(NSDictionary *transition, id<MSIMETextClient> client) {
    MSIMEApplyTransitionWithPreeditStyle(transition, client, MSIMEInlinePreeditStylePinyin);
}

NSRange MSIMETextClientCommittedRange(id<MSIMETextClient> client, NSString *text,
                                      uint32_t *preceding, BOOL (^isCurrent)(void)) {
    const NSRange unknown = NSMakeRange(NSNotFound, 0);
    if (preceding) *preceding = 0;
    if (!text.length || text.length > 4096 || !client ||
        (isCurrent && !isCurrent()) || ![client respondsToSelector:@selector(selectedRange)] ||
        ![client respondsToSelector:@selector(attributedSubstringFromRange:)]) return unknown;
    NSRange selected = [client selectedRange];
    if ((isCurrent && !isCurrent()) || selected.location == NSNotFound || selected.length != 0 ||
        selected.location < text.length) return unknown;
    NSRange committed = NSMakeRange(selected.location - text.length, text.length);
    NSString *actual = [client attributedSubstringFromRange:committed].string;
    if ((isCurrent && !isCurrent()) || ![actual isEqualToString:text]) return unknown;
    const uint32_t scalar = MSIMETextClientScalarBefore(client, committed.location, isCurrent);
    if (isCurrent && !isCurrent()) return unknown;
    if (preceding) *preceding = scalar;
    return committed;
}
