#pragma once
#import <Foundation/Foundation.h>
#import "../core/SharedVoicePreferences.h"
#import "VoiceProviderOptions.h"

// 一次识别的查询：识别服务、凭据、整理设置，全部取自 NSUserDefaults 里镜像的共享偏好。
// 录音时的控制器和听写历史里的「重新听写」用同一份，两边不会走偏。
static inline NSDictionary *MSIMEVoiceRecognitionQuery(NSUserDefaults *defaults, uint64_t generation, BOOL providerSocketAvailable) {
    NSString *language = [defaults stringForKey:@"MSIMEClientVoiceLanguage"] ?: @"zh-CN";
    NSDictionary *query = @{ @"language": language.lowercaseString, @"generation": @(generation), @"stream": @(MSIMEVoiceStreamInlinePreeditEnabled(defaults)), @"asr_provider": MSIMEVoiceEffectiveASRProvider([defaults stringForKey:@"MSIMEClientVoiceASRProvider"]), @"asr_endpoint": [defaults stringForKey:@"MSIMEClientVoiceASREndpoint"] ?: @"", @"asr_model": [defaults stringForKey:@"MSIMEClientVoiceASRModel"] ?: @"", @"asr_model_path": [defaults stringForKey:@"MSIMEClientVoiceASRModelPath"] ?: @"", @"asr_token": [defaults stringForKey:@"MSIMEClientVoiceASRToken"] ?: @"", @"doubao_boosting_table_id": [defaults stringForKey:@"MSIMEClientVoiceDoubaoBoostingTableID"] ?: @"", @"asr_app_key": [defaults stringForKey:@"MSIMEClientVoiceDoubaoAppKey"] ?: @"", @"asr_resource_id": [defaults stringForKey:@"MSIMEClientVoiceDoubaoResourceID"] ?: @"", @"polish_enabled": @([defaults boolForKey:@"MSIMEClientVoicePolish"]), @"polish_prompt_id": [defaults stringForKey:@"MSIMEClientVoicePolishPromptID"] ?: @"cleanup", @"polish_provider": [defaults stringForKey:@"MSIMEClientVoicePolishProvider"] ?: MSIMEVoicePolishDefaultProvider, @"polish_model": [defaults stringForKey:@"MSIMEClientVoicePolishModel"] ?: @"", @"polish_endpoint": [defaults stringForKey:@"MSIMEClientVoicePolishEndpoint"] ?: @"", @"polish_token": [defaults stringForKey:@"MSIMEClientVoicePolishToken"] ?: @"", @"polish_thinking": [defaults stringForKey:@"MSIMEClientVoicePolishThinking"] ?: @"", @"polish_prompt_custom_1": [defaults stringForKey:@"MSIMEClientVoicePolishPromptCustom1"] ?: @"", @"polish_prompt_custom_2": [defaults stringForKey:@"MSIMEClientVoicePolishPromptCustom2"] ?: @"", @"polish_prompt_custom_3": [defaults stringForKey:@"MSIMEClientVoicePolishPromptCustom3"] ?: @"" };
    query = MSIMEVoiceProviderOptions(query, defaults);
    if (MSIMEVoiceUsesTextStream(query[@"asr_provider"], providerSocketAvailable)) query = MSIMEVoiceTextStreamPolishOptions(query, defaults);
    return query;
}
