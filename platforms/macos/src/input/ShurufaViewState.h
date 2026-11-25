#pragma once

#import <CoreFoundation/CoreFoundation.h>
#import <Foundation/Foundation.h>
#include "msime_client.h"
#include "../../../shared/input/EnglishModeOutput.h"
#include <vector>

namespace shurufa::mac {

inline bool StringsEqual(NSString *a, NSString *b) {
    return a == b || (a && b && [a isEqualToString:b]);
}

inline NSString *SliceToNSString(ShurufaStringSlice slice, NSString *fallback = @"") {
    if (!slice.ptr || slice.len == 0) return fallback;
    return [[NSString alloc] initWithBytes:slice.ptr length:slice.len encoding:NSUTF8StringEncoding] ?: fallback;
}

inline BOOL StrictBool(id value) {
    return [value isKindOfClass:NSNumber.class] &&
           CFGetTypeID((__bridge CFTypeRef)value) == CFBooleanGetTypeID() &&
           [(NSNumber *)value boolValue];
}

inline NSInteger StrictInteger(id value, NSInteger fallback) {
    if (![value isKindOfClass:NSNumber.class] ||
        CFGetTypeID((__bridge CFTypeRef)value) == CFBooleanGetTypeID() ||
        CFNumberIsFloatType((__bridge CFNumberRef)value)) return fallback;
    return [(NSNumber *)value integerValue];
}

inline BOOL StrictUnsigned64(id value, uint64_t *result) {
    if (![value isKindOfClass:NSNumber.class] ||
        CFGetTypeID((__bridge CFTypeRef)value) == CFBooleanGetTypeID() ||
        CFNumberIsFloatType((__bridge CFNumberRef)value)) return NO;
    NSNumber *number = (NSNumber *)value;
    if ([number compare:@0] == NSOrderedAscending) return NO;
    uint64_t parsed = number.unsignedLongLongValue;
    if ([number compare:@(parsed)] != NSOrderedSame) return NO;
    if (result) *result = parsed;
    return YES;
}

inline BOOL StrictUnsignedValue(id value, NSUInteger *result) {
    uint64_t parsed = 0;
    if (!StrictUnsigned64(value, &parsed) || parsed > NSUIntegerMax) return NO;
    if (result) *result = static_cast<NSUInteger>(parsed);
    return YES;
}

struct CandidateRow {
    uint64_t session = 0;
    uint64_t generation = 0;
    NSUInteger index = 0;
    bool hasIdentity = false;
    NSString *text = @"";
    NSString *code = @"";
    NSString *annotation = @"";
    NSString *translation = nil;
    NSString *scriptAnnotation = nil;
    uint8_t source = 0;
    uint8_t fixedPosition = 0;
    bool corrected = false;
    bool highlighted = false;
    bool translationReserved = false;

    mutable NSDictionary *cachedId = nil;
    mutable NSDictionary *cachedRow = nil;

    bool isPlaceholder() const { return source == 12 || source == 15; }
    bool isFixedPosition() const { return fixedPosition > 0 && fixedPosition <= 255; }

    NSDictionary *idDictionary() const {
        if (!hasIdentity) return nil;
        if (!cachedId) {
            cachedId = @{@"session": @(session), @"generation": @(generation), @"index": @(index)};
        }
        return cachedId;
    }

    NSDictionary *rowDictionary() const {
        if (cachedRow) return cachedRow;
        NSMutableDictionary *row = [NSMutableDictionary dictionaryWithCapacity:10];
        if (hasIdentity) row[@"id"] = idDictionary();
        row[@"text"] = text ?: @"";
        row[@"code"] = code ?: @"";
        row[@"annotation"] = annotation ?: @"";
        row[@"source"] = @(source);
        row[@"corrected"] = @(corrected);
        row[@"highlighted"] = @(highlighted);
        row[@"translation_reserved"] = @(translationReserved);
        if (fixedPosition > 0) row[@"fixed_position"] = @(fixedPosition);
        if (translation) row[@"translation"] = translation;
        if (scriptAnnotation) row[@"script_annotation"] = scriptAnnotation;
        cachedRow = [row copy];
        return cachedRow;
    }

    bool equalIgnoringTranslation(const CandidateRow &other) const {
        return session == other.session && generation == other.generation && index == other.index &&
               hasIdentity == other.hasIdentity && source == other.source &&
               fixedPosition == other.fixedPosition && corrected == other.corrected &&
               highlighted == other.highlighted && translationReserved == other.translationReserved &&
               (text == other.text || [text isEqualToString:other.text]) &&
               (code == other.code || [code isEqualToString:other.code]) &&
               (annotation == other.annotation || [annotation isEqualToString:other.annotation]) &&
               (scriptAnnotation == other.scriptAnnotation || [scriptAnnotation isEqualToString:other.scriptAnnotation]);
    }

    bool operator==(const CandidateRow &other) const {
        return equalIgnoringTranslation(other) &&
               (translation == other.translation || [translation isEqualToString:other.translation]);
    }

    static CandidateRow fromSnapshotItem(const ShurufaCandidateItem &item) {
        CandidateRow row;
        row.session = item.session;
        row.generation = item.generation;
        row.index = static_cast<NSUInteger>(item.index);
        row.hasIdentity = true;
        row.text = SliceToNSString(item.text);
        row.code = SliceToNSString(item.code);
        row.annotation = SliceToNSString(item.annotation);
        row.translation = item.has_translation ? SliceToNSString(item.translation) : nil;
        row.source = item.source;
        row.fixedPosition = item.fixed_position;
        row.corrected = item.corrected;
        row.highlighted = item.highlighted;
        row.translationReserved = item.translation_reserved;
        return row;
    }

    static CandidateRow fromDictionary(NSDictionary *dict) {
        CandidateRow row;
        if (![dict isKindOfClass:NSDictionary.class]) return row;
        row.cachedRow = [dict copy];
        id rawId = dict[@"id"];
        if ([rawId isKindOfClass:NSDictionary.class]) {
            row.cachedId = [rawId copy];
            uint64_t sid = 0, gen = 0;
            NSUInteger idx = 0;
            if (StrictUnsigned64(rawId[@"session"], &sid) &&
                StrictUnsigned64(rawId[@"generation"], &gen) &&
                StrictUnsignedValue(rawId[@"index"], &idx)) {
                row.session = sid;
                row.generation = gen;
                row.index = idx;
                row.hasIdentity = true;
            }
        }
        row.text = [dict[@"text"] isKindOfClass:NSString.class] ? dict[@"text"] : @"";
        row.code = [dict[@"code"] isKindOfClass:NSString.class] ? dict[@"code"] : @"";
        row.annotation = [dict[@"annotation"] isKindOfClass:NSString.class] ? dict[@"annotation"] : @"";
        row.translation = [dict[@"translation"] isKindOfClass:NSString.class] ? dict[@"translation"] : nil;
        row.scriptAnnotation = [dict[@"script_annotation"] isKindOfClass:NSString.class] ? dict[@"script_annotation"] : nil;
        const NSInteger src = StrictInteger(dict[@"source"], 0);
        row.source = (src >= 0 && src <= 255) ? static_cast<uint8_t>(src) : 0;
        const NSInteger fixed = StrictInteger(dict[@"fixed_position"], 0);
        row.fixedPosition = (fixed >= 0 && fixed <= 255) ? static_cast<uint8_t>(fixed) : 0;
        row.corrected = StrictBool(dict[@"corrected"]);
        row.highlighted = StrictBool(dict[@"highlighted"]);
        row.translationReserved = StrictBool(dict[@"translation_reserved"]);
        return row;
    }
};

/// 强类型输入法视图状态：直接由 `ShurufaViewSnapshot` 填充，热路径不构建 `NSDictionary`。
struct ViewState {
    bool valid = false;
    bool identityValid = false;
    bool generationValid = false;
    bool pagingValid = false;
    uint64_t session = 0;
    uint64_t generation = 0;
    uint8_t scheme = 0;
    uint8_t characterWidth = 0;
    bool prediction = false;
    bool chineseText = false;
    bool scriptConversion = false;
    bool microsoftShuangpin = false;
    bool dedicatedEnglish = false;
    bool focused = false;
    bool caretEditSuffix = false;
    bool provisional = false;
    bool candidateListOpen = false;
    bool nineKey = false;
    ShurufaPunctuationMarks punctuationMarks{};
    NSUInteger caretPosition = 0;
    NSUInteger conversionFocusStart = 0;
    NSUInteger conversionFocusEnd = 0;
    NSUInteger page = 0;
    NSUInteger pageSize = 0;
    NSUInteger pageCount = 0;
    NSString *shuangpinProfile = @"";
    NSString *localMode = @"none";
    NSString *spellingSymbols = @"";
    NSString *preedit = @"";
    NSString *phrasePrefix = @"";
    NSString *reading = @"";
    NSString *displayReading = @"";
    NSString *editingText = @"";
    NSString *conversion = @"";
    NSString *caretEditPrefix = @"";
    std::vector<CandidateRow> candidates;

    mutable NSDictionary *cachedDictionary = nil;
    mutable NSDictionary *cachedPageIdentity = nil;

    bool hasComposition() const {
        return valid && (editingText.length > 0 || !candidates.empty() || phrasePrefix.length > 0);
    }

    bool hasEditableText() const { return valid && editingText.length > 0; }

    bool schemeRulesApply() const {
        return valid && !dedicatedEnglish && (!localMode.length || [localMode isEqualToString:@"none"]);
    }

    bool isKoreanHanjaListOpen() const { return valid && candidateListOpen && scheme == 4; }

    bool showsGlosses() const { return scheme != 9; }

    bool spellingSymbol(unichar character) const {
        if (!valid || character == 0 || character > 0x7F || !spellingSymbols.length) return false;
        return [spellingSymbols rangeOfString:[NSString stringWithCharacters:&character length:1]].location != NSNotFound;
    }

    bool spellingSymbolString(NSString *characters) const {
        return characters.length == 1 && spellingSymbol([characters characterAtIndex:0]);
    }

    msime::input::PunctuationMarks inputPunctuationMarks() const {
        return msime::input::PunctuationMarks{
            punctuationMarks.quotes == 1,
            punctuationMarks.brackets == 1,
            punctuationMarks.enumeration_comma_backslash,
            punctuationMarks.enumeration_comma_slash,
        };
    }

    const CandidateRow *highlightedCandidate() const {
        for (const auto &candidate : candidates) {
            if (!candidate.isPlaceholder() && candidate.highlighted) return &candidate;
        }
        return nullptr;
    }

    bool matchesCandidateIdentity(NSDictionary *identifier) const {
        if (!valid || !focused || !identityValid || ![identifier isKindOfClass:NSDictionary.class]) return false;
        uint64_t sid = 0, gen = 0;
        NSUInteger idx = 0;
        return StrictUnsigned64(identifier[@"session"], &sid) &&
               StrictUnsigned64(identifier[@"generation"], &gen) &&
               StrictUnsignedValue(identifier[@"index"], &idx) &&
               sid == session && gen == generation;
    }

    bool sameExpandedComposition(const ViewState &other) const {
        return (editingText == other.editingText || [editingText isEqualToString:other.editingText]) &&
               (preedit == other.preedit || [preedit isEqualToString:other.preedit]) &&
               caretPosition == other.caretPosition &&
               (phrasePrefix == other.phrasePrefix || [phrasePrefix isEqualToString:other.phrasePrefix]);
    }

    bool equalNonCandidateFields(const ViewState &other) const {
        return valid == other.valid && session == other.session && generation == other.generation &&
               scheme == other.scheme && characterWidth == other.characterWidth &&
               prediction == other.prediction && chineseText == other.chineseText &&
               scriptConversion == other.scriptConversion &&
               microsoftShuangpin == other.microsoftShuangpin &&
               dedicatedEnglish == other.dedicatedEnglish && focused == other.focused &&
               caretEditSuffix == other.caretEditSuffix && provisional == other.provisional &&
               candidateListOpen == other.candidateListOpen && nineKey == other.nineKey &&
               punctuationMarks.quotes == other.punctuationMarks.quotes &&
               punctuationMarks.brackets == other.punctuationMarks.brackets &&
               punctuationMarks.enumeration_comma_backslash == other.punctuationMarks.enumeration_comma_backslash &&
               punctuationMarks.enumeration_comma_slash == other.punctuationMarks.enumeration_comma_slash &&
               caretPosition == other.caretPosition &&
               conversionFocusStart == other.conversionFocusStart &&
               conversionFocusEnd == other.conversionFocusEnd &&
               page == other.page && pageSize == other.pageSize && pageCount == other.pageCount &&
               StringsEqual(shuangpinProfile, other.shuangpinProfile) &&
               StringsEqual(localMode, other.localMode) &&
               StringsEqual(spellingSymbols, other.spellingSymbols) &&
               StringsEqual(preedit, other.preedit) &&
               StringsEqual(phrasePrefix, other.phrasePrefix) &&
               StringsEqual(reading, other.reading) &&
               StringsEqual(displayReading, other.displayReading) &&
               StringsEqual(editingText, other.editingText) &&
               StringsEqual(conversion, other.conversion) &&
               StringsEqual(caretEditPrefix, other.caretEditPrefix);
    }

    NSString *effectivePreedit() const { return preedit ?: editingText; }

    bool operator==(const ViewState &other) const {
        return equalNonCandidateFields(other) && candidates == other.candidates;
    }

    static bool onlyTranslationsChanged(const ViewState &before, const ViewState &after) {
        if (!before.valid || !after.valid) return false;
        if (before.candidates.empty() || before.candidates.size() != after.candidates.size()) return false;
        if (!before.equalNonCandidateFields(after)) return false;
        bool changed = false;
        for (size_t i = 0; i < before.candidates.size(); ++i) {
            if (!before.candidates[i].equalIgnoringTranslation(after.candidates[i])) return false;
            NSString *oldT = before.candidates[i].translation ?: @"";
            NSString *newT = after.candidates[i].translation ?: @"";
            if (![oldT isEqualToString:newT]) changed = true;
        }
        return changed;
    }

    NSArray<NSDictionary *> *visibleCandidatesArray() const {
        NSMutableArray<NSDictionary *> *result = [NSMutableArray arrayWithCapacity:candidates.size()];
        for (const auto &candidate : candidates) {
            if (!candidate.isPlaceholder()) [result addObject:candidate.rowDictionary()];
        }
        return result;
    }

    NSDictionary *pageIdentityDictionary() const {
        if (!cachedPageIdentity) {
            cachedPageIdentity = @{
                @"session": @(session),
                @"generation": @(generation),
                @"page": @(page),
            };
        }
        return cachedPageIdentity;
    }

    NSDictionary *toDictionary() const {
        if (!valid) return nil;
        if (cachedDictionary) return cachedDictionary;
        NSMutableArray<NSDictionary *> *items = [NSMutableArray arrayWithCapacity:candidates.size()];
        for (const auto &candidate : candidates) [items addObject:candidate.rowDictionary()];
        NSMutableDictionary *dict = [@{
            @"session": @(session),
            @"generation": @(generation),
            @"scheme": @(scheme),
            @"chinese_text": @(chineseText),
            @"script_conversion": @(scriptConversion),
            @"microsoft_shuangpin": @(microsoftShuangpin),
            @"character_width": characterWidth == 1 ? @"Fullwidth" : @"Halfwidth",
            @"punctuation_marks": @{
                @"quotes": punctuationMarks.quotes == 1 ? @"corner" : @"curly",
                @"brackets": punctuationMarks.brackets == 1 ? @"hollow" : @"solid",
                @"enumeration_comma_backslash": @(punctuationMarks.enumeration_comma_backslash),
                @"enumeration_comma_slash": @(punctuationMarks.enumeration_comma_slash),
            },
            @"shuangpin_profile": shuangpinProfile ?: @"",
            @"local_mode": localMode ?: @"none",
            @"spelling_symbols": spellingSymbols ?: @"",
            @"dedicated_english": @(dedicatedEnglish),
            @"focused": @(focused),
            @"preedit": preedit ?: @"",
            @"reading": reading ?: @"",
            @"display_reading": displayReading ?: @"",
            @"editing_text": editingText ?: @"",
            @"caret_position": @(caretPosition),
            @"conversion": conversion ?: @"",
            @"conversion_focus_start": @(conversionFocusStart),
            @"conversion_focus_end": @(conversionFocusEnd),
            @"page": @(page),
            @"page_size": @(pageSize),
            @"page_count": @(pageCount),
            @"candidates": items,
        } mutableCopy];
        if (prediction) dict[@"prediction"] = @YES;
        if (phrasePrefix.length) dict[@"phrase_prefix"] = phrasePrefix;
        if (caretEditSuffix) dict[@"caret_edit_suffix"] = @YES;
        if (caretEditPrefix.length) dict[@"caret_edit_prefix"] = caretEditPrefix;
        if (provisional) dict[@"provisional"] = @YES;
        if (candidateListOpen) dict[@"candidate_list_open"] = @YES;
        if (nineKey) dict[@"nine_key"] = @YES;
        cachedDictionary = [dict copy];
        return cachedDictionary;
    }

    static ViewState fromSnapshot(const ShurufaViewSnapshot &snap) {
        ViewState state;
        state.valid = true;
        state.identityValid = true;
        state.generationValid = true;
        state.pagingValid = true;
        state.session = snap.session;
        state.generation = snap.generation;
        state.scheme = snap.scheme;
        state.characterWidth = snap.character_width;
        state.prediction = snap.prediction;
        state.chineseText = snap.chinese_text;
        state.scriptConversion = snap.script_conversion;
        state.microsoftShuangpin = snap.microsoft_shuangpin;
        state.dedicatedEnglish = snap.dedicated_english;
        state.focused = snap.focused;
        state.caretEditSuffix = snap.caret_edit_suffix;
        state.provisional = snap.provisional;
        state.punctuationMarks = snap.punctuation_marks;
        state.caretPosition = static_cast<NSUInteger>(snap.caret_position);
        state.conversionFocusStart = static_cast<NSUInteger>(snap.conversion_focus_start);
        state.conversionFocusEnd = static_cast<NSUInteger>(snap.conversion_focus_end);
        state.page = static_cast<NSUInteger>(snap.page);
        state.pageSize = static_cast<NSUInteger>(snap.page_size);
        state.pageCount = static_cast<NSUInteger>(snap.page_count);
        state.shuangpinProfile = SliceToNSString(snap.shuangpin_profile);
        state.localMode = SliceToNSString(snap.local_mode, @"none");
        state.spellingSymbols = SliceToNSString(snap.spelling_symbols);
        state.preedit = SliceToNSString(snap.preedit);
        state.phrasePrefix = SliceToNSString(snap.phrase_prefix);
        state.reading = SliceToNSString(snap.reading);
        state.displayReading = SliceToNSString(snap.display_reading);
        state.editingText = SliceToNSString(snap.editing_text);
        state.conversion = SliceToNSString(snap.conversion);
        state.caretEditPrefix = SliceToNSString(snap.caret_edit_prefix);
        if (snap.candidates && snap.candidates_len > 0) {
            state.candidates.reserve(snap.candidates_len);
            for (size_t i = 0; i < snap.candidates_len; ++i) {
                state.candidates.push_back(CandidateRow::fromSnapshotItem(snap.candidates[i]));
            }
        }
        return state;
    }

    static ViewState fromDictionary(NSDictionary *dict) {
        ViewState state;
        if (![dict isKindOfClass:NSDictionary.class]) return state;
        state.valid = true;
        state.cachedDictionary = [dict copy];
        const BOOL hasSession = StrictUnsigned64(dict[@"session"], &state.session);
        const BOOL hasGeneration = StrictUnsigned64(dict[@"generation"], &state.generation);
        state.generationValid = hasGeneration;
        state.identityValid = hasSession && hasGeneration;
        const NSInteger sc = StrictInteger(dict[@"scheme"], 0);
        state.scheme = (sc >= 0 && sc <= 255) ? static_cast<uint8_t>(sc) : 0;
        id width = dict[@"character_width"];
        state.characterWidth = ([width isKindOfClass:NSString.class] && [width isEqualToString:@"Fullwidth"]) ? 1 : 0;
        state.prediction = StrictBool(dict[@"prediction"]);
        state.chineseText = dict[@"chinese_text"] ? StrictBool(dict[@"chinese_text"]) : false;
        state.scriptConversion = StrictBool(dict[@"script_conversion"]);
        state.microsoftShuangpin = StrictBool(dict[@"microsoft_shuangpin"]);
        state.dedicatedEnglish = StrictBool(dict[@"dedicated_english"]);
        state.focused = StrictBool(dict[@"focused"]);
        state.caretEditSuffix = StrictBool(dict[@"caret_edit_suffix"]);
        state.provisional = StrictBool(dict[@"provisional"]);
        state.candidateListOpen = StrictBool(dict[@"candidate_list_open"]);
        state.nineKey = StrictBool(dict[@"nine_key"]);
        state.punctuationMarks.enumeration_comma_backslash = true;
        id marks = dict[@"punctuation_marks"];
        if ([marks isKindOfClass:NSDictionary.class]) {
            state.punctuationMarks.quotes = [marks[@"quotes"] isEqual:@"corner"] ? 1 : 0;
            state.punctuationMarks.brackets = [marks[@"brackets"] isEqual:@"hollow"] ? 1 : 0;
            state.punctuationMarks.enumeration_comma_backslash = ![marks[@"enumeration_comma_backslash"] isEqual:@NO];
            state.punctuationMarks.enumeration_comma_slash = [marks[@"enumeration_comma_slash"] isEqual:@YES];
        }
        (void)StrictUnsignedValue(dict[@"caret_position"], &state.caretPosition);
        (void)StrictUnsignedValue(dict[@"conversion_focus_start"], &state.conversionFocusStart);
        (void)StrictUnsignedValue(dict[@"conversion_focus_end"], &state.conversionFocusEnd);
        const BOOL hasPage = StrictUnsignedValue(dict[@"page"], &state.page);
        const BOOL hasPageCount = StrictUnsignedValue(dict[@"page_count"], &state.pageCount);
        state.pagingValid = hasPage && hasPageCount;
        (void)StrictUnsignedValue(dict[@"page_size"], &state.pageSize);
        state.shuangpinProfile = [dict[@"shuangpin_profile"] isKindOfClass:NSString.class] ? dict[@"shuangpin_profile"] : @"";
        state.localMode = [dict[@"local_mode"] isKindOfClass:NSString.class] ? dict[@"local_mode"] : @"";
        state.spellingSymbols = [dict[@"spelling_symbols"] isKindOfClass:NSString.class] ? dict[@"spelling_symbols"] : @"";
        state.editingText = [dict[@"editing_text"] isKindOfClass:NSString.class] ? dict[@"editing_text"] : @"";
        state.preedit = [dict[@"preedit"] isKindOfClass:NSString.class] ? dict[@"preedit"] : state.editingText;
        state.phrasePrefix = [dict[@"phrase_prefix"] isKindOfClass:NSString.class] ? dict[@"phrase_prefix"] : @"";
        state.reading = [dict[@"reading"] isKindOfClass:NSString.class] ? dict[@"reading"] : @"";
        state.displayReading = [dict[@"display_reading"] isKindOfClass:NSString.class] ? dict[@"display_reading"] : @"";
        state.conversion = [dict[@"conversion"] isKindOfClass:NSString.class] ? dict[@"conversion"] : @"";
        state.caretEditPrefix = [dict[@"caret_edit_prefix"] isKindOfClass:NSString.class] ? dict[@"caret_edit_prefix"] : @"";
        id rawCandidates = dict[@"candidates"];
        if ([rawCandidates isKindOfClass:NSArray.class]) {
            state.candidates.reserve([(NSArray *)rawCandidates count]);
            for (id entry in (NSArray *)rawCandidates) {
                if ([entry isKindOfClass:NSDictionary.class]) {
                    state.candidates.push_back(CandidateRow::fromDictionary((NSDictionary *)entry));
                }
            }
        }
        return state;
    }
};

} // namespace shurufa::mac
