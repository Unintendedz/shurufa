#pragma once

#import <Foundation/Foundation.h>

// 本进程里获得焦点的输入控制器是否还有没完成的输入：组字中的拼音或普通英文（editing_text）、正在录或正在识别的语音。只在主线程调用。
BOOL MSIMEInputControllerHasUnfinishedInput(void);
