cmake_minimum_required(VERSION 3.25)
set(CMAKE_OSX_DEPLOYMENT_TARGET "13.0" CACHE STRING "Minimum macOS version; match the Cargo build")
project(MSIMEClientMacOS LANGUAGES CXX OBJCXX)
enable_testing()
include_directories("${CMAKE_CURRENT_SOURCE_DIR}/src/voice" "${CMAKE_CURRENT_SOURCE_DIR}/src/candidate" "${CMAKE_CURRENT_SOURCE_DIR}/src/input" "${CMAKE_CURRENT_SOURCE_DIR}/src/cloud" "${CMAKE_CURRENT_SOURCE_DIR}/src/dictionary" "${CMAKE_CURRENT_SOURCE_DIR}/src/settings" "${CMAKE_CURRENT_SOURCE_DIR}/src/core")
add_executable(desktop-input-session-test tests/input/DesktopInputSessionTest.mm src/core/DesktopInputSession.mm)
target_compile_options(desktop-input-session-test PRIVATE -fobjc-arc -UNDEBUG -Wall -Wextra -Werror)
target_link_libraries(desktop-input-session-test PRIVATE "-framework AppKit")
add_test(NAME desktop-input-session COMMAND desktop-input-session-test)
add_executable(bounded-file-reader-test tests/core/BoundedFileReaderTest.mm src/core/BoundedFileReader.mm)
target_compile_options(bounded-file-reader-test PRIVATE -fobjc-arc -UNDEBUG -Wall -Wextra -Werror)
target_link_libraries(bounded-file-reader-test PRIVATE "-framework Foundation")
add_test(NAME bounded-file-reader COMMAND bounded-file-reader-test)
add_executable(voice-provider-socket-test tests/voice/VoiceProviderSocketTest.mm)
target_compile_options(voice-provider-socket-test PRIVATE -fobjc-arc -UNDEBUG -Wall -Wextra -Werror)
target_link_libraries(voice-provider-socket-test PRIVATE "-framework Foundation")
add_test(NAME voice-provider-socket COMMAND voice-provider-socket-test)
add_executable(input-mode-hud-panel-test tests/input/InputModeHUDPanelTest.mm src/input/InputModeHUDPanel.mm)
target_compile_options(input-mode-hud-panel-test PRIVATE -fobjc-arc -UNDEBUG -Wall -Wextra -Werror)
target_link_libraries(input-mode-hud-panel-test PRIVATE "-framework AppKit")
# InputModeHUDPanel.mm 经 CandidatePlacement.h 引入 shared/candidate（嵌套命名空间、std::optional），这个目标定义在下面统一设置 C++17 之前，所以单独指定。
set_target_properties(input-mode-hud-panel-test PROPERTIES OBJCXX_STANDARD 17)
add_test(NAME input-mode-hud-panel COMMAND input-mode-hud-panel-test)
# The selected input mode (中, 英, 日 or 한) follows the controller's state; this checks the mapping to the plist's mode identifiers and that a switch made by either side is not echoed back by the other.
add_executable(input-mode-identifiers-test tests/input/InputModeIdentifiersTest.mm)
target_compile_options(input-mode-identifiers-test PRIVATE -fobjc-arc -UNDEBUG -Wall -Wextra -Werror)
# InputModeIdentifiers.h 经 src/core/SystemPathAlias.h 用到 std::filesystem，而这个目标定义在下面统一设置 C++17 之前，所以单独指定。
set_target_properties(input-mode-identifiers-test PROPERTIES OBJCXX_STANDARD 17)
target_link_libraries(input-mode-identifiers-test PRIVATE "-framework Foundation")
add_test(NAME input-mode-identifiers COMMAND input-mode-identifiers-test)
# 中日英循环切换在 macOS 这一侧只做两处翻译：控制器状态 → msime_client_language_cycle 的入参、
# 答复 → 控制器要执行的动作。循环顺序在 crates/client-core/src/language_cycle.rs 的单测里。
add_executable(trilingual-cycle-routing-test tests/input/TrilingualCycleRoutingTest.mm)
target_compile_options(trilingual-cycle-routing-test PRIVATE -fobjc-arc -UNDEBUG -Wall -Wextra -Werror)
set_target_properties(trilingual-cycle-routing-test PROPERTIES OBJCXX_STANDARD 20)
target_link_libraries(trilingual-cycle-routing-test PRIVATE "-framework Foundation")
add_test(NAME trilingual-cycle-routing COMMAND trilingual-cycle-routing-test)
add_executable(desktop-input-interop-test tests/input/DesktopInputInteropTest.mm src/core/DesktopInputSession.mm)
target_compile_options(desktop-input-interop-test PRIVATE -fobjc-arc -UNDEBUG -Wall -Wextra -Werror)
target_link_libraries(desktop-input-interop-test PRIVATE "-framework AppKit")
set(MSIME_PANEL_SESSION_PROBE "" CACHE FILEPATH "Optional Cargo panel_session_probe executable")
if(MSIME_PANEL_SESSION_PROBE)
  add_test(NAME desktop-input-interop COMMAND desktop-input-interop-test "${MSIME_PANEL_SESSION_PROBE}")
endif()
add_subdirectory(../../shared/voice shared-voice)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_OBJCXX_STANDARD 17)
add_executable(desktop-account-cancellation-test tests/core/DesktopAccountCancellationTest.mm src/core/DesktopCloudClipboard.mm src/core/DesktopInputSession.mm)
target_compile_options(desktop-account-cancellation-test PRIVATE -fobjc-arc -UNDEBUG -Wall -Wextra -Werror)
target_link_libraries(desktop-account-cancellation-test PRIVATE "-framework AppKit")
add_test(NAME desktop-account-cancellation COMMAND desktop-account-cancellation-test)
set_tests_properties(desktop-account-cancellation PROPERTIES TIMEOUT 10)
add_executable(desktop-cloud-dictionary-interop-test tests/dictionary/DesktopCloudDictionaryInteropTest.mm src/core/DesktopCloudClipboard.mm src/core/DesktopInputSession.mm)
target_compile_options(desktop-cloud-dictionary-interop-test PRIVATE -fobjc-arc -UNDEBUG -Wall -Wextra -Werror)
target_link_libraries(desktop-cloud-dictionary-interop-test PRIVATE "-framework AppKit")
set(MSIME_CLOUD_DICTIONARY_PROBE "" CACHE FILEPATH "Optional Cargo cloud_dictionary_probe executable")
if(MSIME_CLOUD_DICTIONARY_PROBE)
  add_test(NAME desktop-cloud-dictionary-interop COMMAND desktop-cloud-dictionary-interop-test "${MSIME_CLOUD_DICTIONARY_PROBE}")
  set_tests_properties(desktop-cloud-dictionary-interop PROPERTIES TIMEOUT 25)
endif()
add_executable(desktop-cloud-clipboard-interop-test tests/clipboard/DesktopCloudClipboardInteropTest.mm src/core/DesktopCloudClipboard.mm src/core/DesktopInputSession.mm)
target_compile_options(desktop-cloud-clipboard-interop-test PRIVATE -fobjc-arc -UNDEBUG -Wall -Wextra -Werror)
target_link_libraries(desktop-cloud-clipboard-interop-test PRIVATE "-framework AppKit")
set(MSIME_CLOUD_CLIPBOARD_PROBE "" CACHE FILEPATH "Optional Cargo cloud_clipboard_probe executable")
if(MSIME_CLOUD_CLIPBOARD_PROBE)
  add_test(NAME desktop-cloud-clipboard-interop COMMAND desktop-cloud-clipboard-interop-test "${MSIME_CLOUD_CLIPBOARD_PROBE}")
  set_tests_properties(desktop-cloud-clipboard-interop PROPERTIES TIMEOUT 25)
endif()
add_executable(desktop-settings-launcher-test tests/settings/DesktopSettingsLauncherTest.mm)
target_compile_options(desktop-settings-launcher-test PRIVATE -fobjc-arc -UNDEBUG -Wall -Wextra -Werror)
target_link_libraries(desktop-settings-launcher-test PRIVATE "-framework AppKit")
add_test(NAME desktop-settings-launcher COMMAND desktop-settings-launcher-test)
add_executable(shared-uninstaller-test tests/core/UninstallerTest.mm src/core/Uninstaller.mm)
target_compile_options(shared-uninstaller-test PRIVATE -fobjc-arc -UNDEBUG -Wall -Wextra -Werror)
target_link_libraries(shared-uninstaller-test PRIVATE "-framework Foundation" "-framework Security")
add_test(NAME shared-uninstaller COMMAND shared-uninstaller-test)
set(MSIME_HOST_LIBRARY "${CMAKE_CURRENT_SOURCE_DIR}/../../target/debug/libmsime_host_api.a" CACHE FILEPATH "Rust host static library for this architecture")
if(NOT EXISTS "${MSIME_HOST_LIBRARY}")
  message(FATAL_ERROR "Build msime-host-api with Cargo first")
endif()
find_package(SQLite3 REQUIRED)
if(TARGET SQLite3::SQLite3)
  set(SQLITE_TARGET SQLite3::SQLite3)
else()
  set(SQLITE_TARGET SQLite::SQLite3)
endif()
add_library(apple-client STATIC ../../shared/apple/MSIMEClientSession.mm ../../shared/apple/TextClient.mm)
target_compile_options(apple-client PRIVATE -fobjc-arc -Wall -Wextra -Werror)
target_include_directories(apple-client PUBLIC ../../shared/apple src ../../crates/host-api/include)
# AppKit for the marked text attributes: a composition carrying an already chosen phrase piece is
# drawn as two clauses, and NSUnderlineStyle* and the marked clause attribute live there. The iOS
# build of these same sources compiles that block out - UIKit's document proxy takes a plain string.
# CoreAudio and AudioToolbox for host-api's microphone capture (cpal), which every consumer of the archive links whether or not it records.
# reqwest 的 system-proxy 功能通过 SystemConfiguration 读取系统代理设置。
set(MSIME_HOST_LIBRARY_FRAMEWORKS "-framework Foundation" "-framework Security" "-framework CoreText" "-framework CoreAudio" "-framework AudioToolbox" "-framework SystemConfiguration")
target_link_libraries(apple-client PUBLIC "${MSIME_HOST_LIBRARY}" ${SQLITE_TARGET} ${MSIME_HOST_LIBRARY_FRAMEWORKS} "-framework AppKit")
if(TARGET shared-doubao-auth)
  # The Rust archive needs the same Apple/system dependencies in shared ABI tests.
  target_link_libraries(shared-doubao-auth PRIVATE ${SQLITE_TARGET} ${MSIME_HOST_LIBRARY_FRAMEWORKS})
endif()
# src/cloud 的 CloudClipboardClient、CloudClipboardWindowController、CloudDictionaryClient、CloudURLSession 是账号和云端改走 Rust（#8201）以前的 Objective-C 实现，输入法里已经没有代码引用它们，不再编进输入法；它们仍由下面的 cloud-clipboard-deletion、cloud-clipboard-selection、cloud-dictionary-client 三个测试目标编译，scripts/test-macos-cloud-query-encoding.py 和 test-macos-cloud-redirect-policy.py 也读它们的源码，所以文件留着，删不删由负责人定。同一批里没人引用的 CloudFileTransfer 和 src/core/Account{AuthClient,Keychain,SessionManager,WindowController} 已经删掉。
add_executable(MSIMEClientInputMethod MACOSX_BUNDLE src/input/input_method_main.mm src/settings/PreferencesWindowController.mm src/core/SupportWindowController.mm src/voice/VoiceInputService.mm src/voice/VoiceWaveOverlay.mm src/voice/VoiceCuePlayer.mm src/voice/VoiceAudioMuter.mm src/voice/VoiceSettings.mm src/voice/VoiceProviderSettings.mm src/voice/VoiceArchive.mm src/voice/VoiceHistoryView.mm src/voice/TextStreamSupport.mm src/voice/OpenLessImport.mm src/input/InputController.mm src/input/InputModeHUDPanel.mm src/input/InputModeStatusItem.mm src/candidate/CandidateSkinAppearance.mm src/settings/AppearancePreferences.mm src/core/DiagnosticLog.cpp src/core/BoundedFileReader.mm src/candidate/CandidateSkin.cpp src/candidate/CandidateSkinPreviewView.mm src/candidate/SkinSettingsView.mm src/core/ChineseTextConversion.mm src/settings/ShuangpinKeymapPanel.mm src/input/InputSourceRegistration.mm src/core/FloatingToolbarPanel.mm src/core/UpdateController.mm src/dictionary/DictionaryWindowController.mm src/core/ClientDictionaryRuntime.mm src/dictionary/DictionaryInstaller.mm src/core/Uninstaller.mm)
target_sources(MSIMEClientInputMethod PRIVATE src/core/AISettingsWindow.mm src/core/ScreenKeyboardPanel.mm src/cloud/CloudCandidateRequest.mm src/core/CustomTranslationBatch.mm src/voice/VoicePCMBuffer.mm src/voice/HTTPVoiceRequest.mm)
target_sources(MSIMEClientInputMethod PRIVATE src/voice/DoubaoVoiceRequest.mm src/voice/LocalVoiceRequest.mm src/voice/VoiceContextCapture.mm src/voice/LocalPolishServer.mm src/voice/VoicePolishClient.mm src/voice/VoicePipelineHelper.mm src/voice/VoiceEditTracker.mm src/voice/VoiceLearnedToast.mm src/voice/VoiceRecoveryToast.mm)
target_link_libraries(MSIMEClientInputMethod PRIVATE "-framework Security")
# 所有编译 VoicePolishClient.mm 的目标（宿主、控制器测试、voice-polish-client-test）均链接 apple-client，统一静态绑定 Rust C ABI。
set_source_files_properties(src/voice/VoicePolishClient.mm PROPERTIES COMPILE_DEFINITIONS "MSIME_VOICE_POLISH_USE_RUST_FFI=1")
target_sources(MSIMEClientInputMethod PRIVATE src/core/DesktopInputSession.mm)
target_sources(MSIMEClientInputMethod PRIVATE src/core/DesktopCloudClipboard.mm)
target_link_libraries(MSIMEClientInputMethod PRIVATE "-framework ApplicationServices" "-framework CoreGraphics" "-framework CoreText" "-framework UniformTypeIdentifiers")
# 将 macOS 26 端侧 SpeechAnalyzer（Swift）静态编译进输入法主二进制，不再构建或加载 MSIMEBackend.dylib。
find_program(MSIME_XCRUN xcrun REQUIRED)
execute_process(COMMAND "${MSIME_XCRUN}" -f swiftc
  OUTPUT_VARIABLE MSIME_SWIFTC_PATH OUTPUT_STRIP_TRAILING_WHITESPACE ERROR_QUIET)
get_filename_component(MSIME_SWIFTC_BIN_DIR "${MSIME_SWIFTC_PATH}" DIRECTORY)
get_filename_component(MSIME_SWIFTC_TOOLCHAIN_ROOT "${MSIME_SWIFTC_BIN_DIR}" DIRECTORY)
set(MSIME_SWIFT_RUNTIME_LIB_DIR "${MSIME_SWIFTC_TOOLCHAIN_ROOT}/lib/swift/macosx")
set(MSIME_SPEECH_ANALYZER_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/src/voice/BackendSpeechAnalyzer.swift")
set(MSIME_BUNDLE_ARCHITECTURES ${CMAKE_OSX_ARCHITECTURES})
if(NOT MSIME_BUNDLE_ARCHITECTURES)
  set(MSIME_BUNDLE_ARCHITECTURES ${CMAKE_HOST_SYSTEM_PROCESSOR})
endif()
string(REPLACE ";" " " MSIME_SWIFT_ARCHS "${MSIME_BUNDLE_ARCHITECTURES}")
set(MSIME_SPEECH_ANALYZER_OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/libShurufaSpeechAnalyzer.a")
set(MSIME_SWIFT_CONFIGURATION_STAMP "${CMAKE_CURRENT_BINARY_DIR}/swift-speech-configuration.txt")
file(CONFIGURE OUTPUT "${MSIME_SWIFT_CONFIGURATION_STAMP}" CONTENT "${CMAKE_BUILD_TYPE}:${MSIME_SWIFT_ARCHS}\n")
set(MSIME_SPEECH_ANALYZER_SLICES)
foreach(arch IN LISTS MSIME_BUNDLE_ARCHITECTURES)
  set(slice_obj "${CMAKE_CURRENT_BINARY_DIR}/speech-analyzer-${arch}.o")
  set(slice_lib "${CMAKE_CURRENT_BINARY_DIR}/libShurufaSpeechAnalyzer-${arch}.a")
  add_custom_command(OUTPUT "${slice_lib}"
    COMMAND "${MSIME_XCRUN}" swiftc -parse-as-library -emit-object
      -module-name ShurufaSpeechAnalyzer
      "$<IF:$<CONFIG:Release>,-Osize,-Onone>"
      "$<$<CONFIG:Release>:-wmo>"
      -target "${arch}-apple-macosx${CMAKE_OSX_DEPLOYMENT_TARGET}"
      "${MSIME_SPEECH_ANALYZER_SOURCE}" -o "${slice_obj}"
    COMMAND "${MSIME_XCRUN}" ar rcs "${slice_lib}" "${slice_obj}"
    DEPENDS "${MSIME_SPEECH_ANALYZER_SOURCE}" "${MSIME_SWIFT_CONFIGURATION_STAMP}"
    COMMAND_EXPAND_LISTS VERBATIM)
  list(APPEND MSIME_SPEECH_ANALYZER_SLICES "${slice_lib}")
endforeach()
add_custom_command(OUTPUT "${MSIME_SPEECH_ANALYZER_OUTPUT}"
  COMMAND "${MSIME_XCRUN}" lipo -create ${MSIME_SPEECH_ANALYZER_SLICES} -output "${MSIME_SPEECH_ANALYZER_OUTPUT}"
  DEPENDS ${MSIME_SPEECH_ANALYZER_SLICES}
  VERBATIM)
add_custom_target(macos-speech-analyzer DEPENDS "${MSIME_SPEECH_ANALYZER_OUTPUT}")
add_dependencies(MSIMEClientInputMethod macos-speech-analyzer)
target_include_directories(MSIMEClientInputMethod PRIVATE src)
target_compile_options(MSIMEClientInputMethod PRIVATE -fobjc-arc -Wall -Wextra -Werror)
target_link_libraries(MSIMEClientInputMethod PRIVATE apple-client msime-voice-providers "-framework AppKit" "-framework InputMethodKit" "-framework Carbon" "-framework AVFoundation" "-framework Speech")
target_link_options(MSIMEClientInputMethod PRIVATE
  "LINKER:-force_load,${MSIME_SPEECH_ANALYZER_OUTPUT}"
  "-L${MSIME_SWIFT_RUNTIME_LIB_DIR}"
  "LINKER:-rpath,/usr/lib/swift")
# 单按 Caps Lock 切换中英文时用 IOKit 的 IOHIDSetModifierLockState 撤回大写锁定（系统框架，不需要额外权限）。
target_link_libraries(MSIMEClientInputMethod PRIVATE "-framework IOKit")
# build-number.txt 是本地构建的默认构建号；发布工作流传入 MSIME_BUILD_NUMBER，缺省是所构建提交的提交时间距 2026-01-01T00:00:00Z 的分钟数（release-macos.yml），同一版本号的正式版、测试版和每日构建按代码先后排序，同一个提交重跑得到同一个构建号。以前用工作流运行 ID 对 999999999 取模，约两个月回绕一次，回绕后的构建会被 Sparkle 当成降级；改用时间之后的构建号比那时发出的小，同一版本号内要等下一次版本号递增才能盖过它们（见 .agents/notes/implemented/feature/2026-10-25-macos-update-channels.md）。CFBundleVersion 不直接用它，而是把版本号编进去：((主 × 1000 + 次) × 1000 + 修订) × 1,000,000,000 + 构建号。这样发布构建号可以在 1 到 999999999 之间增长，且跨版本保持单调；0.54.0 的本地构建 1 是 5400000000000000001。Sparkle 的安装进程（Autoupdate 的 SUPlainInstaller）只用标准比较器比新旧 bundle 的 CFBundleVersion，新的更小就当降级拒绝装，不经过输入法里的任何委托。设置应用把构建号按 u64 解析（apps/desktop/src-tauri/src/platform/macos/macos_input_source.rs 的 BundleVersion）。
set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${CMAKE_CURRENT_SOURCE_DIR}/build-number.txt" "${CMAKE_CURRENT_SOURCE_DIR}/version.txt")
set(MSIME_BUILD_NUMBER "" CACHE STRING "macOS bundle 构建号；缺省使用 build-number.txt")
if(NOT MSIME_BUILD_NUMBER)
  file(STRINGS "${CMAKE_CURRENT_SOURCE_DIR}/build-number.txt" MSIME_BUILD_NUMBER LIMIT_COUNT 1)
endif()
string(STRIP "${MSIME_BUILD_NUMBER}" MSIME_BUILD_NUMBER)
if(NOT MSIME_BUILD_NUMBER MATCHES "^[1-9][0-9]*$" OR MSIME_BUILD_NUMBER GREATER 999999999)
  message(FATAL_ERROR "MSIME_BUILD_NUMBER must hold an integer from 1 to 999999999, not '${MSIME_BUILD_NUMBER}'")
endif()
file(STRINGS "${CMAKE_CURRENT_SOURCE_DIR}/version.txt" MSIME_MACOS_VERSION LIMIT_COUNT 1)
string(STRIP "${MSIME_MACOS_VERSION}" MSIME_MACOS_VERSION)
if(NOT MSIME_MACOS_VERSION MATCHES "^([0-9]+)\\.([0-9][0-9]?[0-9]?)\\.([0-9][0-9]?[0-9]?)$")
  message(FATAL_ERROR "platforms/macos/version.txt must be MAJOR.MINOR.PATCH with MINOR and PATCH below 1000, not '${MSIME_MACOS_VERSION}'")
endif()
math(EXPR MACOSX_BUNDLE_BUNDLE_VERSION "((${CMAKE_MATCH_1} * 1000 + ${CMAKE_MATCH_2}) * 1000 + ${CMAKE_MATCH_3}) * 1000000000 + ${MSIME_BUILD_NUMBER}")
message(STATUS "Bundle build number: ${MACOSX_BUNDLE_BUNDLE_VERSION}")
# version.txt is the only place the version is written: Info.plist.in takes CFBundleShortVersionString from this variable, and the sources that need a fallback for an unbundled executable read MSIME_MACOS_VERSION_STRING, so a release bumps version.txt (and build-number.txt) and nothing else.
set(MACOSX_BUNDLE_SHORT_VERSION_STRING "${MSIME_MACOS_VERSION}")
add_compile_definitions(MSIME_MACOS_VERSION_STRING="${MSIME_MACOS_VERSION}")
set(MSIME_INPUT_SOURCE_REVISION "" CACHE STRING "写入输入法 bundle 的源码修订")
# CMake 的正则不支持 `{m,n}` 计数量词，写进正则的长度限制会让任何修订都不匹配，所以长度单独检查。
string(LENGTH "${MSIME_INPUT_SOURCE_REVISION}" MSIME_INPUT_SOURCE_REVISION_LENGTH)
if(MSIME_INPUT_SOURCE_REVISION AND (NOT MSIME_INPUT_SOURCE_REVISION MATCHES "^[0-9A-Fa-f]+$" OR
   MSIME_INPUT_SOURCE_REVISION_LENGTH LESS 7 OR MSIME_INPUT_SOURCE_REVISION_LENGTH GREATER 64))
  message(FATAL_ERROR "MSIME_INPUT_SOURCE_REVISION must be a hexadecimal source revision")
endif()
set(MSIME_INPUT_SOURCE_REVISION_PLIST "")
if(MSIME_INPUT_SOURCE_REVISION)
  set(MSIME_INPUT_SOURCE_REVISION_PLIST "<key>MSIMEInputSourceRevision</key><string>${MSIME_INPUT_SOURCE_REVISION}</string>")
endif()
# 版本（edition）是 shared/contracts/editions.json 里的 id。full 用 Info.plist.in 和 resources/<语言>.lproj/InfoPlist.strings 原样构建，产物与引入版本之前相同；其他版本由 scripts/edition_bundle.py 在配置时从同样的模板生成该版本的 Info.plist 和 InfoPlist.strings（bundle id、模式、版本键），可执行文件也改成该版本的名字。原生代码在运行时从 Info.plist 读版本身份（src/core/EditionIdentity.h），所以除了这几个文件，各版本编出来的东西完全一样；发布包只编 full，再用同一个脚本把 bundle 改成各个版本（package-release.sh）。
set(MSIME_EDITION "full" CACHE STRING "Edition id from shared/contracts/editions.json")
find_package(Python3 REQUIRED COMPONENTS Interpreter)
set(MSIME_EDITION_SCRIPT "${CMAKE_CURRENT_SOURCE_DIR}/scripts/edition_bundle.py")
# Sparkle 只在发布包里打开：package-release.sh 拿得到签更新的私钥时传入 appcast 所在的目录，这里拼出本版本的 SUFeedURL（appcast-<版本 id>.xml，edition_bundle.py 把 full 的改成别的版本的；它始终是稳定版的 feed，用户在设置页选了测试版或每日构建时，UpdateController 由它推出同目录下的 appcast-<通道>-<版本 id>.xml，见 src/core/UpdateChannelFeed.h），写入 sparkle-public-key.txt 里的公钥，并打开每天一次的后台检查与静默更新。开发构建不带这些键：带了的话，本机编的输入法会被正式版覆盖。没有 feed 时 UpdateController 走官网下载页（MSIMEUpdateRouteForHost）。
option(MSIME_ENABLE_SPARKLE "Link the Sparkle application updater" ON)
if(MSIME_EDITION STREQUAL "shurufa")
  set(MSIME_ENABLE_SPARKLE OFF CACHE BOOL "Link the Sparkle application updater" FORCE)
endif()
set(MSIME_SPARKLE_APPCAST_BASE_URL "" CACHE STRING "URL prefix of the appcast-<edition>.xml feeds; empty leaves Sparkle off")
set(MSIME_SPARKLE_PLIST_KEYS "")
if(MSIME_ENABLE_SPARKLE AND MSIME_SPARKLE_APPCAST_BASE_URL)
  if(NOT MSIME_SPARKLE_APPCAST_BASE_URL MATCHES "^https://.*/$")
    message(FATAL_ERROR "MSIME_SPARKLE_APPCAST_BASE_URL must be an https URL ending in '/', not '${MSIME_SPARKLE_APPCAST_BASE_URL}'")
  endif()
  set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${CMAKE_CURRENT_SOURCE_DIR}/sparkle-public-key.txt")
  file(STRINGS "${CMAKE_CURRENT_SOURCE_DIR}/sparkle-public-key.txt" MSIME_SPARKLE_PUBLIC_KEY LIMIT_COUNT 1)
  string(STRIP "${MSIME_SPARKLE_PUBLIC_KEY}" MSIME_SPARKLE_PUBLIC_KEY)
  if(NOT MSIME_SPARKLE_PUBLIC_KEY MATCHES "^[A-Za-z0-9+/]+=*$")
    message(FATAL_ERROR "platforms/macos/sparkle-public-key.txt must hold the base64 EdDSA public key that generate_keys prints")
  endif()
  # SURequireSignedFeed 要求 appcast 本身也带签名（generate_appcast 用同一把私钥签），SUVerifyUpdateBeforeExtraction 在解压之前验更新包的签名；两项与 MSIME-Apple 时期发出去的 0.48 版一致。SUAutomaticallyUpdate：在后台下载，输入法进程退出时安装，不弹窗。输入法是 LSBackgroundOnly，Sparkle 的标准提醒窗口会从正在打字的应用那里抢走焦点；手动「检查更新…」照常显示 Sparkle 的界面。
  string(CONCAT MSIME_SPARKLE_PLIST_KEYS
    "<key>SUFeedURL</key><string>${MSIME_SPARKLE_APPCAST_BASE_URL}appcast-${MSIME_EDITION}.xml</string>\n"
    "<key>SUPublicEDKey</key><string>${MSIME_SPARKLE_PUBLIC_KEY}</string>\n"
    "<key>SURequireSignedFeed</key><true/>\n"
    "<key>SUVerifyUpdateBeforeExtraction</key><true/>\n"
    "<key>SUEnableAutomaticChecks</key><true/>\n"
    "<key>SUAutomaticallyUpdate</key><true/>\n"
    "<key>SUScheduledCheckInterval</key><integer>86400</integer>\n")
  message(STATUS "Sparkle feed: ${MSIME_SPARKLE_APPCAST_BASE_URL}appcast-${MSIME_EDITION}.xml")
endif()
if(MSIME_EDITION STREQUAL "full")
  set(MSIME_INFO_PLIST_TEMPLATE "${CMAKE_CURRENT_SOURCE_DIR}/Info.plist.in")
  set(MSIME_LOCALIZED_RESOURCES "${CMAKE_CURRENT_SOURCE_DIR}/resources")
  set(MSIME_INPUT_METHOD_NAME "水杉输入法")
else()
  set(MSIME_EDITION_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/edition-${MSIME_EDITION}")
  set(MSIME_INFO_PLIST_TEMPLATE "${MSIME_EDITION_DIRECTORY}/Info.plist.in")
  set(MSIME_LOCALIZED_RESOURCES "${MSIME_EDITION_DIRECTORY}/resources")
  execute_process(
    COMMAND "${Python3_EXECUTABLE}" "${MSIME_EDITION_SCRIPT}" plist --edition "${MSIME_EDITION}"
            --template "${CMAKE_CURRENT_SOURCE_DIR}/Info.plist.in" --output "${MSIME_INFO_PLIST_TEMPLATE}"
    RESULT_VARIABLE MSIME_EDITION_RESULT)
  if(NOT MSIME_EDITION_RESULT EQUAL 0)
    message(FATAL_ERROR "Cannot generate the Info.plist of edition ${MSIME_EDITION}")
  endif()
  foreach(MSIME_BUNDLE_LOCALE zh-Hans en)
    execute_process(
      COMMAND "${Python3_EXECUTABLE}" "${MSIME_EDITION_SCRIPT}" strings --edition "${MSIME_EDITION}"
              --input "${CMAKE_CURRENT_SOURCE_DIR}/resources/${MSIME_BUNDLE_LOCALE}.lproj/InfoPlist.strings"
              --output "${MSIME_LOCALIZED_RESOURCES}/${MSIME_BUNDLE_LOCALE}.lproj/InfoPlist.strings"
      RESULT_VARIABLE MSIME_EDITION_RESULT)
    if(NOT MSIME_EDITION_RESULT EQUAL 0)
      message(FATAL_ERROR "Cannot generate the ${MSIME_BUNDLE_LOCALE} InfoPlist.strings of edition ${MSIME_EDITION}")
    endif()
    set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS
      "${CMAKE_CURRENT_SOURCE_DIR}/resources/${MSIME_BUNDLE_LOCALE}.lproj/InfoPlist.strings")
  endforeach()
  execute_process(
    COMMAND "${Python3_EXECUTABLE}" "${MSIME_EDITION_SCRIPT}" field --edition "${MSIME_EDITION}" input_method_name
    OUTPUT_VARIABLE MSIME_INPUT_METHOD_NAME OUTPUT_STRIP_TRAILING_WHITESPACE
    RESULT_VARIABLE MSIME_EDITION_RESULT)
  if(NOT MSIME_EDITION_RESULT EQUAL 0)
    message(FATAL_ERROR "Cannot read the input method name of edition ${MSIME_EDITION}")
  endif()
  set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS
    "${CMAKE_CURRENT_SOURCE_DIR}/Info.plist.in" "${MSIME_EDITION_SCRIPT}" "${CMAKE_CURRENT_SOURCE_DIR}/../../shared/contracts/editions.json")
endif()
message(STATUS "Edition: ${MSIME_EDITION} (${MSIME_INPUT_METHOD_NAME}.app)")
set_target_properties(MSIMEClientInputMethod PROPERTIES MACOSX_BUNDLE_INFO_PLIST "${MSIME_INFO_PLIST_TEMPLATE}")
set(MSIME_LEGAL_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/Licenses")
file(MAKE_DIRECTORY "${MSIME_LEGAL_DIRECTORY}")
configure_file("${CMAKE_CURRENT_SOURCE_DIR}/../../LICENSE" "${MSIME_LEGAL_DIRECTORY}/GPL-3.0.txt" COPYONLY)
configure_file("${CMAKE_CURRENT_SOURCE_DIR}/resources/Licenses/THIRD_PARTY_NOTICES.txt" "${MSIME_LEGAL_DIRECTORY}/THIRD_PARTY_NOTICES.txt" COPYONLY)
# 固定词库发布里的词库数据。
configure_file("${CMAKE_CURRENT_SOURCE_DIR}/../../resources/licenses/msime-engine-dictionary-NOTICE.md" "${MSIME_LEGAL_DIRECTORY}/msime-engine-dictionary-NOTICE.md" COPYONLY)
# The place names `@` mode offers, compiled into the engine from modood/Administrative-divisions-of-China (WTFPL).
configure_file("${CMAKE_CURRENT_SOURCE_DIR}/../../resources/licenses/Administrative-divisions-of-China-WTFPL.txt" "${MSIME_LEGAL_DIRECTORY}/Administrative-divisions-of-China-WTFPL.txt" COPYONLY)
# 语法模型查询逻辑的移植声明随引擎分发。
configure_file("${CMAKE_CURRENT_SOURCE_DIR}/../../resources/licenses/librime-octagram-BSD-3-Clause.txt" "${MSIME_LEGAL_DIRECTORY}/librime-octagram-BSD-3-Clause.txt" COPYONLY)
# 符号模式（S 模式）的分组码表来自 rime-wanxiang 的 wanxiang_symbols.yaml（CC BY 4.0）；第 3(a) 条要求分发改编数据时附上署名、许可证地址和改动说明，这份文件就是。
configure_file("${CMAKE_CURRENT_SOURCE_DIR}/../../resources/licenses/rime-wanxiang-CC-BY-4.0.txt" "${MSIME_LEGAL_DIRECTORY}/rime-wanxiang-CC-BY-4.0.txt" COPYONLY)
configure_file("${CMAKE_CURRENT_SOURCE_DIR}/../../resources/licenses/Unicode-License-V3.txt" "${MSIME_LEGAL_DIRECTORY}/Unicode-License-V3.txt" COPYONLY)
# 简繁转换内嵌 OpenCC 词表，Apache-2.0 原文随宿主分发。
configure_file("${CMAKE_CURRENT_SOURCE_DIR}/../../resources/licenses/OpenCC-Apache-2.0.txt" "${MSIME_LEGAL_DIRECTORY}/OpenCC-Apache-2.0.txt" COPYONLY)
configure_file("${CMAKE_CURRENT_SOURCE_DIR}/../../resources/licenses/world-clock-NOTICE.txt" "${MSIME_LEGAL_DIRECTORY}/world-clock-NOTICE.txt" COPYONLY)
# symphonia and triple_buffer, which host-api links in for key sounds and music, and rink-core, which the engine links in for V-mode unit conversion, are MPL-2.0: the licence text ships with the binary, and THIRD_PARTY_NOTICES.txt says where their source is.
configure_file("${CMAKE_CURRENT_SOURCE_DIR}/../../resources/licenses/MPL-2.0.txt" "${MSIME_LEGAL_DIRECTORY}/MPL-2.0.txt" COPYONLY)
# The on-device speech runtime staged into Contents/Frameworks below. The upstream archive carries no license file, so the texts come from the copies the other desktops ship: sherpa-onnx's beside its vendored C header, ONNX Runtime's pinned under platforms/linux.
configure_file("${CMAKE_CURRENT_SOURCE_DIR}/../../shared/voice/third_party/sherpa-onnx/LICENSE" "${MSIME_LEGAL_DIRECTORY}/sherpa-onnx-Apache-2.0.txt" COPYONLY)
configure_file("${CMAKE_CURRENT_SOURCE_DIR}/../../resources/licenses/onnxruntime-MIT.txt" "${MSIME_LEGAL_DIRECTORY}/onnxruntime-MIT.txt" COPYONLY)
configure_file("${CMAKE_CURRENT_SOURCE_DIR}/../../resources/licenses/onnxruntime-ThirdPartyNotices.txt" "${MSIME_LEGAL_DIRECTORY}/onnxruntime-ThirdPartyNotices.txt" COPYONLY)
configure_file("${CMAKE_CURRENT_SOURCE_DIR}/../../shared/voice/third_party/cppcodec/LICENSE" "${MSIME_LEGAL_DIRECTORY}/cppcodec-MIT.txt" COPYONLY)
file(GLOB MSIME_LEGAL_RESOURCES CONFIGURE_DEPENDS "${MSIME_LEGAL_DIRECTORY}/*")
target_sources(MSIMEClientInputMethod PRIVATE ${MSIME_LEGAL_RESOURCES})
set_source_files_properties(${MSIME_LEGAL_RESOURCES} PROPERTIES MACOSX_PACKAGE_LOCATION "Resources/Licenses")
set_target_properties(MSIMEClientInputMethod PROPERTIES OUTPUT_NAME "${MSIME_INPUT_METHOD_NAME}")
# The preferences window runs under NSApplicationActivationPolicyRegular, so the bundle appears in
# the Dock and needs an icon; without CFBundleIconFile plus the file itself it shows the generic one.
set(MSIME_ICON_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/resources")
if(MSIME_EDITION STREQUAL "shurufa")
  set(MSIME_ICON_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/resources/shurufa")
endif()
target_sources(MSIMEClientInputMethod PRIVATE "${MSIME_ICON_ROOT}/MSIMEClientInputMethod.icns")
set_source_files_properties("${MSIME_ICON_ROOT}/MSIMEClientInputMethod.icns"
  PROPERTIES MACOSX_PACKAGE_LOCATION Resources)
# 输入菜单自己的图标，是模板 TIFF 而不是应用图标：bundle 的标志（切换 HUD 也显示它），以及十一个输入模式各自的大字图标。标志为什么不能和应用图标是同一个文件，见 Info.plist.in 里 tsInputMethodIconFileKey 旁的注释。
set(MSIME_MENU_ICON_RESOURCES
  "${MSIME_ICON_ROOT}/MSIMEClientInputMethodMenuIcon.tiff"
  "${MSIME_ICON_ROOT}/MSIMEClientInputMethodMenuIconChinese.tiff"
  "${MSIME_ICON_ROOT}/MSIMEClientInputMethodMenuIconShuangpin.tiff"
  "${MSIME_ICON_ROOT}/MSIMEClientInputMethodMenuIconWubi.tiff"
  "${MSIME_ICON_ROOT}/MSIMEClientInputMethodMenuIconCantonese.tiff"
  "${MSIME_ICON_ROOT}/MSIMEClientInputMethodMenuIconZhuyin.tiff"
  "${MSIME_ICON_ROOT}/MSIMEClientInputMethodMenuIconJapanese.tiff"
  "${MSIME_ICON_ROOT}/MSIMEClientInputMethodMenuIconKorean.tiff"
  "${MSIME_ICON_ROOT}/MSIMEClientInputMethodMenuIconVietnamese.tiff"
  "${MSIME_ICON_ROOT}/MSIMEClientInputMethodMenuIconTibetan.tiff"
  "${MSIME_ICON_ROOT}/MSIMEClientInputMethodMenuIconStroke.tiff"
  "${MSIME_ICON_ROOT}/MSIMEClientInputMethodMenuIconEnglish.tiff")
target_sources(MSIMEClientInputMethod PRIVATE ${MSIME_MENU_ICON_RESOURCES})
set_source_files_properties(${MSIME_MENU_ICON_RESOURCES} PROPERTIES MACOSX_PACKAGE_LOCATION Resources)
# The voice start/end cues are the product's own, not system sounds. They are staged straight from the Windows installer's copy rather than adding another copy to the tree; VoiceCuePlayer looks them up in Resources/audios, mirroring the assets/audios layout Windows installs.
set(MSIME_VOICE_CUE_RESOURCES
  "${CMAKE_CURRENT_SOURCE_DIR}/resources/audios/start.mp3"
  "${CMAKE_CURRENT_SOURCE_DIR}/resources/audios/end.mp3")
target_sources(MSIMEClientInputMethod PRIVATE ${MSIME_VOICE_CUE_RESOURCES})
set_source_files_properties(${MSIME_VOICE_CUE_RESOURCES} PROPERTIES MACOSX_PACKAGE_LOCATION "Resources/audios")
# 内置音效包由 scripts/generate_sound_packs.py 合成，每个包一个文件夹，放在 Resources/sound-packs 下：输入法和设置页把这个目录作为内置包根目录告诉 client-services。放在 EngineResources 旁边而不是里面，因为那个目录必须恰好只装词库锁所固定的内容。
# 那里的每个文件夹都会被暂存，所以生成器新增的包不必在这里再点一次名就会随之发布。
file(GLOB MSIME_SOUND_PACK_DIRECTORIES LIST_DIRECTORIES true CONFIGURE_DEPENDS "${CMAKE_CURRENT_SOURCE_DIR}/../../resources/sound-packs/*")
foreach(MSIME_SOUND_PACK_DIRECTORY ${MSIME_SOUND_PACK_DIRECTORIES})
  if(NOT IS_DIRECTORY "${MSIME_SOUND_PACK_DIRECTORY}")
    continue()
  endif()
  get_filename_component(MSIME_SOUND_PACK "${MSIME_SOUND_PACK_DIRECTORY}" NAME)
  file(GLOB MSIME_SOUND_PACK_FILES CONFIGURE_DEPENDS "${MSIME_SOUND_PACK_DIRECTORY}/*")
  target_sources(MSIMEClientInputMethod PRIVATE ${MSIME_SOUND_PACK_FILES})
  set_source_files_properties(${MSIME_SOUND_PACK_FILES} PROPERTIES MACOSX_PACKAGE_LOCATION "Resources/sound-packs/${MSIME_SOUND_PACK}")
endforeach()
# 随桌面安装包分发的官方插件包（plugins/official-packs.json 登记的脚本插件和短语表），由 scripts/build-official-plugins.sh 构建到 target/plugin-packs/<kind>/<id>/，原样放进 Resources/plugin-packs/<kind>/<id>/。它在内置音效包旁边：宿主只传 sound_packs，host-api 和设置页从那里推出 plugin-packs，所以输入法代码不用改。bundle 的签名封住这些文件，随 app 更新整体替换；它们默认停用，脚本插件要授权、短语表要启用才生效。
# 目录不在时（没先跑构建脚本的开发构建）不带官方包；发版设 MSIME_REQUIRE_OFFICIAL_PLUGINS=ON，那时目录不在就配置失败。
set(MSIME_PLUGIN_PACKS_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../../target/plugin-packs" CACHE PATH "Official plugin packs built by scripts/build-official-plugins.sh, staged into Resources/plugin-packs when present")
option(MSIME_REQUIRE_OFFICIAL_PLUGINS "Fail the configure when MSIME_PLUGIN_PACKS_DIR holds no official plugin packs" OFF)
set(MSIME_STAGED_PLUGIN_PACKS "")
if(IS_DIRECTORY "${MSIME_PLUGIN_PACKS_DIR}")
  file(GLOB MSIME_PLUGIN_PACK_DIRECTORIES LIST_DIRECTORIES true CONFIGURE_DEPENDS "${MSIME_PLUGIN_PACKS_DIR}/*/*")
  foreach(MSIME_PLUGIN_PACK_DIRECTORY ${MSIME_PLUGIN_PACK_DIRECTORIES})
    if(NOT IS_DIRECTORY "${MSIME_PLUGIN_PACK_DIRECTORY}")
      continue()
    endif()
    get_filename_component(MSIME_PLUGIN_PACK "${MSIME_PLUGIN_PACK_DIRECTORY}" NAME)
    get_filename_component(MSIME_PLUGIN_PACK_KIND_DIRECTORY "${MSIME_PLUGIN_PACK_DIRECTORY}" DIRECTORY)
    get_filename_component(MSIME_PLUGIN_PACK_KIND "${MSIME_PLUGIN_PACK_KIND_DIRECTORY}" NAME)
    file(GLOB MSIME_PLUGIN_PACK_FILES CONFIGURE_DEPENDS "${MSIME_PLUGIN_PACK_DIRECTORY}/*")
    target_sources(MSIMEClientInputMethod PRIVATE ${MSIME_PLUGIN_PACK_FILES})
    set_source_files_properties(${MSIME_PLUGIN_PACK_FILES} PROPERTIES MACOSX_PACKAGE_LOCATION "Resources/plugin-packs/${MSIME_PLUGIN_PACK_KIND}/${MSIME_PLUGIN_PACK}")
    list(APPEND MSIME_STAGED_PLUGIN_PACKS "${MSIME_PLUGIN_PACK_KIND}/${MSIME_PLUGIN_PACK}")
  endforeach()
endif()
if(MSIME_REQUIRE_OFFICIAL_PLUGINS AND NOT MSIME_STAGED_PLUGIN_PACKS)
  message(FATAL_ERROR "MSIME_REQUIRE_OFFICIAL_PLUGINS is on but ${MSIME_PLUGIN_PACKS_DIR} holds no plugin packs; run scripts/build-official-plugins.sh first")
endif()
# LaunchServices takes the Dock and menu bar name from a localized InfoPlist.strings, not from the
# CFBundleDisplayName in the plist; without these the Dock falls back to the executable's filename.
# The TIS identifier keys name the entry in the system input menu.
#
# The location has to name the .lproj directory. Every locale's file is called InfoPlist.strings, so a
# bare `Resources` flattens them into one file and the last locale overwrites the rest - the bundle then
# carries only the English names and the input menu loses 水杉输入法. bundle-contents checks each locale.
foreach(MSIME_BUNDLE_LOCALE zh-Hans en)
  target_sources(MSIMEClientInputMethod
    PRIVATE "${MSIME_LOCALIZED_RESOURCES}/${MSIME_BUNDLE_LOCALE}.lproj/InfoPlist.strings")
  set_source_files_properties("${MSIME_LOCALIZED_RESOURCES}/${MSIME_BUNDLE_LOCALE}.lproj/InfoPlist.strings"
    PROPERTIES MACOSX_PACKAGE_LOCATION "Resources/${MSIME_BUNDLE_LOCALE}.lproj")
endforeach()
# On-device recognition runs in its own process: the input method spawns Contents/MacOS/msime-voice-local (see src/voice/LocalVoiceRequest.h) and the helper loads the pinned sherpa-onnx runtime from Contents/Frameworks, where shared/voice/LocalAsr.cpp looks beside an executable in Contents/MacOS. The runtime is the prebuilt upstream library pinned in resources/voice-runtime.lock.json; scripts/fetch_voice_runtime.py downloads and verifies it once into MSIME_VOICE_RUNTIME_DIR and reuses its cached archive afterwards, and the stamp reruns it whenever the lock or the script changes.
find_package(Python3 REQUIRED COMPONENTS Interpreter)
set(MSIME_VOICE_RUNTIME_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../../target/voice-runtime/macos" CACHE PATH "Where scripts/fetch_voice_runtime.py puts the macOS sherpa-onnx runtime")
set(MSIME_VOICE_RUNTIME_LIBRARY "${MSIME_VOICE_RUNTIME_DIR}/libsherpa-onnx-c-api.dylib")
set(MSIME_VOICE_RUNTIME_STAMP "${CMAKE_CURRENT_BINARY_DIR}/voice-runtime.stamp")
add_custom_command(OUTPUT "${MSIME_VOICE_RUNTIME_STAMP}"
  COMMAND "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/../../scripts/fetch_voice_runtime.py" --platform macos --out "${MSIME_VOICE_RUNTIME_DIR}"
  COMMAND "${CMAKE_COMMAND}" -E touch "${MSIME_VOICE_RUNTIME_STAMP}"
  DEPENDS "${CMAKE_CURRENT_SOURCE_DIR}/../../resources/voice-runtime.lock.json" "${CMAKE_CURRENT_SOURCE_DIR}/../../scripts/fetch_voice_runtime.py"
  COMMENT "Fetching the pinned sherpa-onnx runtime for on-device speech recognition"
  VERBATIM)
add_custom_target(macos-voice-runtime DEPENDS "${MSIME_VOICE_RUNTIME_STAMP}")
# 发布包在 resources/voice-runtime-macos.lock.json 固定了重签的运行库、并且用同一个 Team ID 签名时关掉它（package-release.sh）：运行库不进 bundle，由设置应用作为 voice-runtime-macos 资源包下载，输入法以 --runtime 交给辅助进程（src/voice/LocalVoiceRequest.h）。开发构建照旧内置，签名身份不同的开发者也能直接用本地识别。
option(MSIME_BUNDLE_VOICE_RUNTIME "Stage the sherpa-onnx runtime into Contents/Frameworks; off leaves it to the voice-runtime-macos resource pack" ON)
add_dependencies(MSIMEClientInputMethod msime-voice-local)
set(MSIME_LOCAL_VOICE_HELPER_NAME "msime-voice-local")
if(MSIME_EDITION STREQUAL "shurufa")
  execute_process(COMMAND "${Python3_EXECUTABLE}" "${MSIME_EDITION_SCRIPT}" field
    --edition "${MSIME_EDITION}" voice_helper_name
    OUTPUT_VARIABLE MSIME_LOCAL_VOICE_HELPER_NAME OUTPUT_STRIP_TRAILING_WHITESPACE
    COMMAND_ERROR_IS_FATAL ANY)
  set(MSIME_SHURUFA_RESOURCES "${CMAKE_CURRENT_SOURCE_DIR}/../../target/macos/EngineResources")
  if(NOT EXISTS "${MSIME_SHURUFA_RESOURCES}/msime-pinyin.db")
    message(FATAL_ERROR "Stage the Shurufa core dictionaries before configuring its bundle")
  endif()
  file(GLOB_RECURSE MSIME_SHURUFA_RESOURCE_FILES CONFIGURE_DEPENDS "${MSIME_SHURUFA_RESOURCES}/*")
  foreach(resource IN LISTS MSIME_SHURUFA_RESOURCE_FILES)
    file(RELATIVE_PATH relative "${MSIME_SHURUFA_RESOURCES}" "${resource}")
    get_filename_component(directory "${relative}" DIRECTORY)
    target_sources(MSIMEClientInputMethod PRIVATE "${resource}")
    set_source_files_properties("${resource}" PROPERTIES MACOSX_PACKAGE_LOCATION "Resources/EngineResources/${directory}")
  endforeach()
endif()
# 本地流式识别（TextStream）：输入法 spawn Contents/Resources/textstream/shurufa-textstream，它在自身目录里找 worker；install-runtime.sh 随包，由设置窗口调用，把 Python 运行环境和模型装进用户的 Application Support。
set(MSIME_TEXTSTREAM_FILES shurufa-textstream textstream_worker.py install-runtime.sh requirements.lock LICENSE-TextStream)
foreach(MSIME_TEXTSTREAM_FILE IN LISTS MSIME_TEXTSTREAM_FILES)
  add_custom_command(TARGET MSIMEClientInputMethod POST_BUILD
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different "${CMAKE_CURRENT_SOURCE_DIR}/textstream/${MSIME_TEXTSTREAM_FILE}" "$<TARGET_BUNDLE_DIR:MSIMEClientInputMethod>/Contents/Resources/textstream/${MSIME_TEXTSTREAM_FILE}")
  set_property(TARGET MSIMEClientInputMethod APPEND PROPERTY LINK_DEPENDS "${CMAKE_CURRENT_SOURCE_DIR}/textstream/${MSIME_TEXTSTREAM_FILE}")
endforeach()
add_custom_command(TARGET MSIMEClientInputMethod POST_BUILD
  COMMAND "${CMAKE_COMMAND}" -E make_directory "$<TARGET_BUNDLE_DIR:MSIMEClientInputMethod>/Contents/Frameworks"
  COMMAND "${CMAKE_COMMAND}" -E rm -f "$<TARGET_BUNDLE_DIR:MSIMEClientInputMethod>/Contents/Frameworks/MSIMEBackend.dylib"
  COMMAND "${CMAKE_COMMAND}" -E copy_if_different "$<TARGET_FILE:msime-voice-local>" "$<TARGET_BUNDLE_DIR:MSIMEClientInputMethod>/Contents/MacOS/${MSIME_LOCAL_VOICE_HELPER_NAME}")
if(MSIME_BUNDLE_VOICE_RUNTIME)
  add_dependencies(MSIMEClientInputMethod macos-voice-runtime)
  # 上游的 sherpa-onnx 运行库是 x86_64 + arm64 的通用二进制（约 55 MB）。拷进 bundle 时只抽出 MSIME_BUNDLE_ARCHITECTURES 里的架构：universal 包两种都留，开发构建只编宿主架构时省下另一半。lipo 会破坏上游签名，install.sh 和 package-release.sh 之后都会用 --deep 重新签名整个 bundle。
  set(MSIME_VOICE_RUNTIME_EXTRACT)
  foreach(MSIME_BUNDLE_ARCHITECTURE IN LISTS MSIME_BUNDLE_ARCHITECTURES)
    list(APPEND MSIME_VOICE_RUNTIME_EXTRACT -extract "${MSIME_BUNDLE_ARCHITECTURE}")
  endforeach()
  add_custom_command(TARGET MSIMEClientInputMethod POST_BUILD
    COMMAND lipo "${MSIME_VOICE_RUNTIME_LIBRARY}" ${MSIME_VOICE_RUNTIME_EXTRACT} -output "$<TARGET_BUNDLE_DIR:MSIMEClientInputMethod>/Contents/Frameworks/libsherpa-onnx-c-api.dylib")
else()
  # 关掉之后重新构建同一个构建目录时，上一次拷进去的运行库还在 bundle 里，删掉它。
  add_custom_command(TARGET MSIMEClientInputMethod POST_BUILD
    COMMAND "${CMAKE_COMMAND}" -E rm -f "$<TARGET_BUNDLE_DIR:MSIMEClientInputMethod>/Contents/Frameworks/libsherpa-onnx-c-api.dylib")
endif()
set(MSIME_OPTIONS_FILE "" CACHE FILEPATH "Optional development runtime configuration; never distribute this file")
if(MSIME_OPTIONS_FILE)
  target_sources(MSIMEClientInputMethod PRIVATE "${MSIME_OPTIONS_FILE}")
  set_source_files_properties("${MSIME_OPTIONS_FILE}" PROPERTIES MACOSX_PACKAGE_LOCATION Resources)
endif()
add_executable(clipboard-preferences-test tests/clipboard/ClipboardPreferencesTest.mm)
target_compile_options(clipboard-preferences-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(clipboard-preferences-test PRIVATE "-framework Foundation")
add_test(NAME clipboard-preferences COMMAND clipboard-preferences-test)
set_tests_properties(clipboard-preferences PROPERTIES LABELS "clipboard-local" TIMEOUT 30)
add_executable(emoji-host-cursor-test tests/emoji/EmojiHostCursorTest.mm)
target_compile_options(emoji-host-cursor-test PRIVATE -fobjc-arc -UNDEBUG -Wall -Wextra -Werror)
target_link_libraries(emoji-host-cursor-test PRIVATE apple-client)
add_test(NAME emoji-host-cursor COMMAND emoji-host-cursor-test)
set_tests_properties(emoji-host-cursor PROPERTIES LABELS "emoji-local" TIMEOUT 30)
add_executable(screen-keyboard-panel-test tests/core/ScreenKeyboardPanelTest.mm src/core/ScreenKeyboardPanel.mm)
target_compile_options(screen-keyboard-panel-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(screen-keyboard-panel-test PRIVATE "-framework AppKit" "-framework CoreGraphics" "-framework Carbon")
add_test(NAME screen-keyboard-panel COMMAND screen-keyboard-panel-test)
add_executable(macos-diagnostic-log-test tests/core/DiagnosticLogTest.cpp src/core/DiagnosticLog.cpp)
target_compile_options(macos-diagnostic-log-test PRIVATE -UNDEBUG -Wall -Wextra -Werror)
target_include_directories(macos-diagnostic-log-test PRIVATE src)
add_test(NAME macos-diagnostic-log COMMAND macos-diagnostic-log-test)
add_executable(system-path-alias-test tests/core/SystemPathAliasTest.cpp)
target_compile_options(system-path-alias-test PRIVATE -UNDEBUG -Wall -Wextra -Werror)
add_test(NAME system-path-alias COMMAND system-path-alias-test)
# 纯逻辑：悬浮工具栏的自动隐藏与阴影判定不碰窗口，所以在没有图形会话的机器上也能跑。
add_executable(floating-toolbar-behavior-test tests/core/FloatingToolbarBehaviorTest.cpp)
target_compile_options(floating-toolbar-behavior-test PRIVATE -UNDEBUG -Wall -Wextra -Werror)
add_test(NAME floating-toolbar-behavior COMMAND floating-toolbar-behavior-test)
add_executable(vim-mode-test tests/core/VimModeTest.cpp)
target_compile_options(vim-mode-test PRIVATE -UNDEBUG -Wall -Wextra -Werror)
add_test(NAME vim-mode COMMAND vim-mode-test)
add_executable(shared-voice-preferences-test tests/voice/SharedVoicePreferencesTest.mm)
target_compile_options(shared-voice-preferences-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(shared-voice-preferences-test PRIVATE "-framework Foundation")
add_test(NAME shared-voice-preferences COMMAND shared-voice-preferences-test)
add_executable(voice-provider-options-test tests/voice/VoiceProviderOptionsTest.mm)
target_compile_options(voice-provider-options-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-provider-options-test PRIVATE "-framework Foundation")
add_test(NAME voice-provider-options COMMAND voice-provider-options-test)
add_executable(voice-text-commit-test tests/voice/VoiceTextCommitTest.mm)
target_compile_options(voice-text-commit-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-text-commit-test PRIVATE "-framework AppKit" "-framework CoreGraphics")
add_test(NAME voice-text-commit COMMAND voice-text-commit-test)
add_executable(voice-deactivation-test tests/voice/VoiceDeactivationTest.mm)
target_compile_options(voice-deactivation-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-deactivation-test PRIVATE "-framework AppKit")
add_test(NAME voice-deactivation COMMAND voice-deactivation-test)
add_executable(voice-pcm-buffer-test tests/voice/VoicePCMBufferTest.mm src/voice/VoicePCMBuffer.mm)
target_compile_options(voice-pcm-buffer-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-pcm-buffer-test PRIVATE "-framework Foundation" "-framework AVFoundation")
add_test(NAME voice-pcm-buffer COMMAND voice-pcm-buffer-test)
add_executable(voice-transcription-lifecycle-test tests/voice/VoiceTranscriptionLifecycleTest.mm src/voice/VoiceInputService.mm src/voice/VoicePCMBuffer.mm)
target_compile_options(voice-transcription-lifecycle-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-transcription-lifecycle-test PRIVATE "-framework Foundation" "-framework AVFoundation" "-framework Speech" "-framework CoreAudio" "-framework AudioToolbox")
add_test(NAME voice-transcription-lifecycle COMMAND voice-transcription-lifecycle-test)
add_executable(voice-capture-device-test tests/voice/VoiceCaptureDeviceTest.mm)
target_compile_options(voice-capture-device-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-capture-device-test PRIVATE "-framework Foundation" "-framework CoreAudio" "-framework AudioToolbox")
add_test(NAME voice-capture-device COMMAND voice-capture-device-test)
add_executable(voice-pcm-capture-test tests/voice/VoicePCMCaptureTest.mm src/voice/VoiceInputService.mm src/voice/VoicePCMBuffer.mm)
target_compile_options(voice-pcm-capture-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-pcm-capture-test PRIVATE "-framework Foundation" "-framework AVFoundation" "-framework Speech" "-framework CoreAudio" "-framework AudioToolbox")
add_test(NAME voice-pcm-capture COMMAND voice-pcm-capture-test)
add_executable(voice-pcm-stream-test tests/voice/VoicePCMStreamTest.mm src/voice/VoiceInputService.mm src/voice/VoicePCMBuffer.mm)
target_compile_options(voice-pcm-stream-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-pcm-stream-test PRIVATE "-framework Foundation" "-framework AVFoundation" "-framework Speech" "-framework AudioToolbox" "-framework CoreAudio")
add_test(NAME voice-pcm-stream COMMAND voice-pcm-stream-test)
add_executable(http-voice-request-test tests/voice/HTTPVoiceRequestTest.mm src/voice/HTTPVoiceRequest.mm)
target_compile_options(http-voice-request-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(http-voice-request-test PRIVATE msime-voice-providers "-framework Foundation")
find_package(Python3 REQUIRED COMPONENTS Interpreter)
add_test(NAME http-voice-request COMMAND "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/voice/http_voice_fixture.py" "$<TARGET_FILE:http-voice-request-test>")
add_executable(voice-failure-messages-test tests/voice/VoiceFailureMessagesTest.mm)
target_compile_options(voice-failure-messages-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-failure-messages-test PRIVATE msime-voice-providers "-framework Foundation")
add_test(NAME voice-failure-messages COMMAND voice-failure-messages-test)
add_executable(doubao-voice-request-test tests/voice/DoubaoVoiceRequestTest.mm src/voice/DoubaoVoiceRequest.mm)
target_compile_options(doubao-voice-request-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(doubao-voice-request-test PRIVATE apple-client "-framework Foundation")
add_test(NAME doubao-voice-request COMMAND "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/voice/doubao_voice_fixture.py" "$<TARGET_FILE:doubao-voice-request-test>")
# Above the fixture's own 60 s client limit plus the 3 s stack sample it takes of a hung client, so a hang reports where it is stuck instead of being killed silently.
set_tests_properties(doubao-voice-request PROPERTIES TIMEOUT 80)
add_executable(local-voice-request-test tests/voice/LocalVoiceRequestTest.mm src/voice/LocalVoiceRequest.mm src/voice/TextStreamSupport.mm)
target_compile_options(local-voice-request-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(local-voice-request-test PRIVATE apple-client "-framework Foundation")
# The protocol against a scripted stand-in for msime-voice-local, so it needs neither the runtime nor a model.
add_test(NAME local-voice-request COMMAND local-voice-request-test)
set_tests_properties(local-voice-request PROPERTIES TIMEOUT 60 ENVIRONMENT
  "MSIME_VOICE_LOCAL_HELPER=${CMAKE_CURRENT_SOURCE_DIR}/tests/voice/local_voice_helper_fixture.py;MSIME_LOCAL_VOICE_FIXTURE_LOG=${CMAKE_CURRENT_BINARY_DIR}/local-voice-request.log")
# With an installed model (resources/local-asr-models.json) and a 16 kHz mono 16-bit recording, also stream that recording through the real helper and the fetched runtime, the way the input method does. Both are downloads, so this is opt-in.
set(MSIME_LOCAL_ASR_TEST_WAV "" CACHE FILEPATH "Optional 16 kHz mono 16-bit WAV for the local-voice-model test")
if(MSIME_LOCAL_ASR_TEST_MODEL AND MSIME_LOCAL_ASR_TEST_WAV)
  add_test(NAME local-voice-model COMMAND local-voice-request-test --model "${MSIME_LOCAL_ASR_TEST_MODEL}" --wav "${MSIME_LOCAL_ASR_TEST_WAV}")
  add_dependencies(local-voice-request-test msime-voice-local macos-voice-runtime)
  set_tests_properties(local-voice-model PROPERTIES TIMEOUT 180 ENVIRONMENT
    "MSIME_VOICE_LOCAL_HELPER=$<TARGET_FILE:msime-voice-local>;MSIME_SHERPA_ONNX_LIBRARY=${MSIME_VOICE_RUNTIME_LIBRARY}")
endif()
add_executable(textstream-request-test tests/voice/TextStreamRequestTest.mm src/voice/LocalVoiceRequest.mm src/voice/TextStreamSupport.mm)
target_compile_options(textstream-request-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(textstream-request-test PRIVATE apple-client "-framework Foundation")
# TextStream 通道对着脚本化的启动器替身：正常路径、取消，以及运行环境不可用时的错误。
foreach(MSIME_TEXTSTREAM_MODE normal unavailable)
  add_test(NAME textstream-request-${MSIME_TEXTSTREAM_MODE} COMMAND textstream-request-test ${MSIME_TEXTSTREAM_MODE})
  set_tests_properties(textstream-request-${MSIME_TEXTSTREAM_MODE} PROPERTIES TIMEOUT 90 ENVIRONMENT
    "MSIME_TEXTSTREAM_LAUNCHER=${CMAKE_CURRENT_SOURCE_DIR}/tests/voice/textstream_launcher_fixture.py;MSIME_TS_FIXTURE_MODE=${MSIME_TEXTSTREAM_MODE};MSIME_TS_FIXTURE_LOG=${CMAKE_CURRENT_BINARY_DIR}/textstream-request-${MSIME_TEXTSTREAM_MODE}.log")
endforeach()
# 真启动器加已安装的运行环境：把合成录音流式识别一遍。没装好时跳过（退出码 77）。
add_test(NAME textstream-real COMMAND textstream-request-test real "${CMAKE_CURRENT_SOURCE_DIR}/textstream/tests/audio/s1.wav")
set_tests_properties(textstream-real PROPERTIES TIMEOUT 180 SKIP_RETURN_CODE 77 ENVIRONMENT
  "MSIME_TEXTSTREAM_LAUNCHER=${CMAKE_CURRENT_SOURCE_DIR}/textstream/shurufa-textstream")
# 随包协议自测：真实的 worker 对着协议跑全套用例，需要已装好的运行环境。
add_test(NAME textstream-protocol COMMAND "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/textstream/tests/test_protocol.py" all)
set_tests_properties(textstream-protocol PROPERTIES TIMEOUT 600 SKIP_RETURN_CODE 77)
add_executable(openless-import-test tests/voice/OpenLessImportTest.mm src/voice/OpenLessImport.mm)
target_compile_options(openless-import-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(openless-import-test PRIVATE "-framework Foundation" "-framework Security")
add_test(NAME openless-import COMMAND openless-import-test)
add_executable(typing-statistics-test tests/core/TypingStatisticsTest.cpp)
target_compile_options(typing-statistics-test PRIVATE -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(typing-statistics-test PRIVATE apple-client)
add_test(NAME typing-statistics COMMAND typing-statistics-test)
add_executable(key-press-statistics-test tests/core/KeyPressStatisticsTest.cpp)
target_compile_options(key-press-statistics-test PRIVATE -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(key-press-statistics-test PRIVATE apple-client)
add_test(NAME key-press-statistics COMMAND key-press-statistics-test)
add_executable(runtime-options-refresh-test tests/core/RuntimeOptionsRefreshTests.mm)
target_compile_options(runtime-options-refresh-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(runtime-options-refresh-test PRIVATE apple-client "-framework Foundation")
add_test(NAME runtime-options-refresh COMMAND runtime-options-refresh-test)
add_executable(typing-statistics-notification-test
    tests/core/TypingStatisticsNotificationTest.mm
    src/core/DictionaryNotification.mm)
target_compile_options(typing-statistics-notification-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(typing-statistics-notification-test PRIVATE "-framework Foundation")
add_test(NAME typing-statistics-notification COMMAND typing-statistics-notification-test)
add_executable(toolbar-visibility-preferences-test tests/settings/ToolbarVisibilityPreferencesTest.mm src/settings/AppearancePreferences.mm src/core/BoundedFileReader.mm src/candidate/CandidateSkin.cpp src/candidate/CandidateSkinPreviewView.mm src/candidate/SkinSettingsView.mm src/settings/PreferencesWindowController.mm src/candidate/CandidateSkinAppearance.mm src/core/AISettingsWindow.mm)
target_compile_options(toolbar-visibility-preferences-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(toolbar-visibility-preferences-test PRIVATE "-framework AppKit")
add_test(NAME toolbar-visibility-preferences COMMAND toolbar-visibility-preferences-test)
add_executable(scheme-round-trip-preferences-test tests/settings/SchemeRoundTripPreferencesTest.mm src/settings/AppearancePreferences.mm src/core/BoundedFileReader.mm src/candidate/CandidateSkin.cpp src/candidate/CandidateSkinPreviewView.mm src/candidate/SkinSettingsView.mm src/settings/PreferencesWindowController.mm src/candidate/CandidateSkinAppearance.mm src/core/AISettingsWindow.mm)
target_compile_options(scheme-round-trip-preferences-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(scheme-round-trip-preferences-test PRIVATE "-framework AppKit")
add_test(NAME scheme-round-trip-preferences COMMAND scheme-round-trip-preferences-test)
add_executable(local-mode-preferences-test tests/settings/LocalModePreferencesTest.mm src/settings/AppearancePreferences.mm src/core/BoundedFileReader.mm src/candidate/CandidateSkin.cpp src/candidate/CandidateSkinPreviewView.mm src/candidate/SkinSettingsView.mm src/candidate/CandidateSkinAppearance.mm)
target_compile_options(local-mode-preferences-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(local-mode-preferences-test PRIVATE apple-client "-framework AppKit")
add_test(NAME local-mode-preferences COMMAND local-mode-preferences-test)
# 候选窗外观的出厂回退值（只读 getter，不建窗口）。回退漂移过一次 `Segoe UI`／字号 16。
add_executable(candidate-appearance-defaults-test tests/settings/CandidateAppearanceDefaultsTest.mm src/settings/AppearancePreferences.mm src/core/BoundedFileReader.mm src/candidate/CandidateSkin.cpp src/candidate/CandidateSkinPreviewView.mm src/candidate/SkinSettingsView.mm src/candidate/CandidateSkinAppearance.mm)
target_compile_options(candidate-appearance-defaults-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(candidate-appearance-defaults-test PRIVATE apple-client "-framework AppKit")
add_test(NAME candidate-appearance-defaults COMMAND candidate-appearance-defaults-test)
# 语音按住录音组合键的出厂回退值（只读 getter，不建窗口）。`hotkey_ralt` 与共享默认漂移过。
add_executable(voice-hold-defaults-test tests/settings/VoiceHoldDefaultsTest.mm src/settings/AppearancePreferences.mm src/core/BoundedFileReader.mm src/candidate/CandidateSkin.cpp src/candidate/CandidateSkinPreviewView.mm src/candidate/SkinSettingsView.mm src/candidate/CandidateSkinAppearance.mm)
target_compile_options(voice-hold-defaults-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-hold-defaults-test PRIVATE apple-client "-framework AppKit")
add_test(NAME voice-hold-defaults COMMAND voice-hold-defaults-test)
add_executable(skin-package-replace-test tests/candidate/SkinPackageReplaceTest.mm src/settings/AppearancePreferences.mm src/core/BoundedFileReader.mm src/candidate/CandidateSkin.cpp src/candidate/CandidateSkinPreviewView.mm src/candidate/SkinSettingsView.mm src/candidate/CandidateSkinAppearance.mm)
target_compile_options(skin-package-replace-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(skin-package-replace-test PRIVATE apple-client "-framework AppKit")
add_test(NAME skin-package-replace COMMAND skin-package-replace-test)
add_executable(wubi-auto-commit-preferences-test tests/settings/WubiAutoCommitPreferencesTest.mm src/settings/AppearancePreferences.mm src/core/BoundedFileReader.mm src/candidate/CandidateSkin.cpp src/candidate/CandidateSkinPreviewView.mm src/candidate/SkinSettingsView.mm src/candidate/CandidateSkinAppearance.mm)
target_compile_options(wubi-auto-commit-preferences-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(wubi-auto-commit-preferences-test PRIVATE apple-client "-framework AppKit")
add_test(NAME wubi-auto-commit-preferences COMMAND wubi-auto-commit-preferences-test)
add_executable(preferences-lifecycle-test tests/settings/PreferencesLifecycleTest.mm src/settings/AppearancePreferences.mm src/core/BoundedFileReader.mm src/candidate/CandidateSkin.cpp src/candidate/CandidateSkinPreviewView.mm src/candidate/SkinSettingsView.mm src/settings/PreferencesWindowController.mm src/candidate/CandidateSkinAppearance.mm src/core/AISettingsWindow.mm)
target_compile_options(preferences-lifecycle-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(preferences-lifecycle-test PRIVATE "-framework AppKit")
add_test(NAME preferences-lifecycle COMMAND preferences-lifecycle-test)
add_executable(cloud-appearance-settings-test tests/settings/CloudAppearanceSettingsTest.mm src/candidate/CandidateSkin.cpp)
target_compile_options(cloud-appearance-settings-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(cloud-appearance-settings-test PRIVATE "-framework Foundation")
add_test(NAME cloud-appearance-settings COMMAND cloud-appearance-settings-test)
add_executable(cloud-clipboard-selection-test tests/clipboard/CloudClipboardSelectionTest.mm src/cloud/CloudClipboardWindowController.mm)
target_compile_options(cloud-clipboard-selection-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(cloud-clipboard-selection-test PRIVATE "-framework AppKit")
add_test(NAME cloud-clipboard-selection COMMAND cloud-clipboard-selection-test)
add_executable(cloud-clipboard-deletion-test tests/clipboard/CloudClipboardDeletionTest.mm src/cloud/CloudClipboardClient.mm src/cloud/CloudURLSession.mm)
target_compile_options(cloud-clipboard-deletion-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(cloud-clipboard-deletion-test PRIVATE "-framework Foundation")
add_test(NAME cloud-clipboard-deletion COMMAND cloud-clipboard-deletion-test)
add_executable(tool-text-return-test tests/core/ToolTextReturnTest.mm)
target_compile_options(tool-text-return-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(tool-text-return-test PRIVATE "-framework Foundation")
add_test(NAME tool-text-return COMMAND tool-text-return-test)
add_executable(tool-application-activation-test tests/core/ToolApplicationActivationTest.mm)
target_compile_options(tool-application-activation-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(tool-application-activation-test PRIVATE "-framework AppKit")
add_test(NAME tool-application-activation COMMAND tool-application-activation-test)
add_compile_definitions(MSIME_ENABLE_SPARKLE=$<BOOL:${MSIME_ENABLE_SPARKLE}>)
set(MSIME_SPARKLE_ROOT "" CACHE PATH "Directory containing the pinned Sparkle.framework (Apple reference: 2.9.6)")
if(MSIME_ENABLE_SPARKLE AND NOT MSIME_SPARKLE_ROOT)
  message(FATAL_ERROR "UpdateController requires Sparkle 2.9.6. Set MSIME_SPARKLE_ROOT to the directory containing Sparkle.framework; see platforms/macos/README.md.")
endif()
if(MSIME_ENABLE_SPARKLE)
  # Do not reuse a framework cached for a different root on reconfiguration.
  unset(MSIME_SPARKLE_FRAMEWORK CACHE)
  find_library(MSIME_SPARKLE_FRAMEWORK Sparkle PATHS "${MSIME_SPARKLE_ROOT}" NO_DEFAULT_PATH REQUIRED)
  execute_process(COMMAND /usr/libexec/PlistBuddy -c Print:CFBundleShortVersionString
    "${MSIME_SPARKLE_FRAMEWORK}/Resources/Info.plist"
    OUTPUT_VARIABLE MSIME_SPARKLE_VERSION OUTPUT_STRIP_TRAILING_WHITESPACE
    ERROR_QUIET RESULT_VARIABLE MSIME_SPARKLE_VERSION_RESULT)
  if(NOT MSIME_SPARKLE_VERSION_RESULT EQUAL 0 OR NOT MSIME_SPARKLE_VERSION STREQUAL "2.9.6")
    message(FATAL_ERROR "MSIME_SPARKLE_ROOT must contain Sparkle 2.9.6; found '${MSIME_SPARKLE_VERSION}'.")
  endif()
  set(MSIME_SPARKLE_COMPILE_OPTIONS "-F${MSIME_SPARKLE_ROOT}")
  target_compile_options(MSIMEClientInputMethod PRIVATE ${MSIME_SPARKLE_COMPILE_OPTIONS})
  target_link_libraries(MSIMEClientInputMethod PRIVATE "${MSIME_SPARKLE_FRAMEWORK}")
  set_property(TARGET MSIMEClientInputMethod APPEND PROPERTY BUILD_RPATH "@executable_path/../Frameworks")
  # ditto, not `cmake -E copy_directory`: the latter follows symlinks, and a macOS framework is mostly
  # symlinks - Headers, Resources, the binary itself all point into Versions/Current. Flattening them
  # tripled the staged framework from 3 MB to 9 MB and, worse, left codesign calling the bundle format
  # ambiguous and refusing to seal it. An input method that cannot be signed cannot be registered as an
  # input source at all, so this decided whether the product could be installed.
  add_custom_command(TARGET MSIMEClientInputMethod POST_BUILD
    COMMAND "${CMAKE_COMMAND}" -E rm -rf
      "$<TARGET_BUNDLE_DIR:MSIMEClientInputMethod>/Contents/Frameworks/Sparkle.framework"
    COMMAND ditto "${MSIME_SPARKLE_FRAMEWORK}"
      "$<TARGET_BUNDLE_DIR:MSIMEClientInputMethod>/Contents/Frameworks/Sparkle.framework")
endif()
add_executable(candidate-skin-test tests/candidate/CandidateSkinTest.cpp src/candidate/CandidateSkin.cpp)
target_include_directories(candidate-skin-test PRIVATE src)
target_compile_options(candidate-skin-test PRIVATE -Wall -Wextra -Werror -UNDEBUG)
add_test(NAME candidate-skin COMMAND candidate-skin-test)
add_executable(wubi-code-hint-test tests/dictionary/WubiCodeHintPolicyTest.cpp)
target_compile_options(wubi-code-hint-test PRIVATE -Wall -Wextra -Werror -UNDEBUG)
add_test(NAME wubi-code-hint COMMAND wubi-code-hint-test)
add_executable(external-skin-test tests/candidate/ExternalSkinTest.cpp src/candidate/CandidateSkin.cpp)
target_include_directories(external-skin-test PRIVATE src)
target_compile_options(external-skin-test PRIVATE -Wall -Wextra -Werror -UNDEBUG)
add_test(NAME external-skin COMMAND external-skin-test)
add_executable(text-client-test tests/core/TextClientTest.mm)
target_compile_options(text-client-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(text-client-test PRIVATE apple-client)
add_test(NAME text-client COMMAND text-client-test)
add_executable(text-client-reentrancy-test tests/core/TextClientReentrancyTest.mm ../../shared/apple/TextClient.mm)
target_include_directories(text-client-reentrancy-test PRIVATE ../../shared/apple ../../crates/host-api/include)
target_compile_options(text-client-reentrancy-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(text-client-reentrancy-test PRIVATE "-framework Foundation" "-framework AppKit")
add_test(NAME text-client-reentrancy COMMAND text-client-reentrancy-test)
add_executable(cloud-candidate-request-test tests/candidate/CloudCandidateRequestTest.mm src/cloud/CloudCandidateRequest.mm)
target_compile_options(cloud-candidate-request-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(cloud-candidate-request-test PRIVATE apple-client "-framework Foundation")
add_test(NAME cloud-candidate-request COMMAND cloud-candidate-request-test)
add_executable(cloud-dictionary-client-test tests/dictionary/CloudDictionaryClientTest.mm src/cloud/CloudDictionaryClient.mm src/cloud/CloudURLSession.mm src/core/BoundedFileReader.mm)
target_compile_options(cloud-dictionary-client-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(cloud-dictionary-client-test PRIVATE "-framework Foundation")
add_test(NAME cloud-dictionary-client COMMAND cloud-dictionary-client-test)
add_executable(custom-translation-batch-test tests/dictionary/CustomTranslationBatchTest.mm src/core/CustomTranslationBatch.mm src/cloud/CloudCandidateRequest.mm)
target_compile_options(custom-translation-batch-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(custom-translation-batch-test PRIVATE apple-client)
add_test(NAME custom-translation-batch COMMAND custom-translation-batch-test)
add_executable(niutrans-translation-test tests/dictionary/NiuTransTranslationTest.mm src/core/CustomTranslationBatch.mm src/cloud/CloudCandidateRequest.mm)
target_compile_options(niutrans-translation-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(niutrans-translation-test PRIVATE apple-client)
add_test(NAME niutrans-translation COMMAND niutrans-translation-test)
add_executable(translation-cache-test tests/dictionary/TranslationCacheTest.mm src/cloud/TranslationCache.mm)
target_compile_options(translation-cache-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(translation-cache-test PRIVATE "-framework Foundation")
add_test(NAME translation-cache COMMAND translation-cache-test)
target_sources(MSIMEClientInputMethod PRIVATE src/cloud/TranslationCache.mm)
add_executable(ai-settings-snapshot-test tests/settings/AISettingsSnapshotTest.mm)
target_compile_options(ai-settings-snapshot-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(ai-settings-snapshot-test PRIVATE "-framework Foundation")
add_test(NAME ai-settings-snapshot COMMAND ai-settings-snapshot-test)
# AI 接口地址策略逐条跑 shared/contracts/ai-endpoint/cases.json，与 client-providers 的权威实现对照。
add_executable(ai-endpoint-policy-test tests/settings/AIEndpointPolicyTest.mm)
target_compile_options(ai-endpoint-policy-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(ai-endpoint-policy-test PRIVATE apple-client "-framework Foundation")
add_test(NAME ai-endpoint-policy COMMAND ai-endpoint-policy-test)
add_executable(ai-settings-window-test tests/settings/AISettingsWindowTest.mm src/core/AISettingsWindow.mm)
target_compile_options(ai-settings-window-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(ai-settings-window-test PRIVATE apple-client "-framework AppKit")
add_test(NAME ai-settings-window COMMAND ai-settings-window-test)
add_executable(translation-settings-test tests/dictionary/TranslationSettingsTest.mm src/cloud/TranslationSettingsWindow.mm)
target_compile_options(translation-settings-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(translation-settings-test PRIVATE apple-client "-framework AppKit")
add_test(NAME translation-settings COMMAND translation-settings-test)
add_executable(chinese-text-conversion-test tests/core/ChineseTextConversionTest.mm src/core/ChineseTextConversion.mm)
target_compile_options(chinese-text-conversion-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(chinese-text-conversion-test PRIVATE apple-client "-framework Foundation")
add_test(NAME chinese-text-conversion COMMAND chinese-text-conversion-test)
add_executable(session-smoke-test ../../shared/apple/tests/session_smoke.mm)
target_compile_options(session-smoke-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_include_directories(session-smoke-test PRIVATE ../../shared/apple ../../crates/host-api/include)
target_link_libraries(session-smoke-test PRIVATE apple-client "-framework Foundation")
add_test(NAME session-smoke COMMAND session-smoke-test)

add_executable(voice-session-test ../../shared/apple/tests/voice_session.mm)
target_compile_options(voice-session-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-session-test PRIVATE apple-client)
add_test(NAME voice-session COMMAND voice-session-test)

add_executable(voice-session-delivery-test tests/voice/VoiceSessionDeliveryTest.mm src/voice/VoiceInputService.mm src/voice/VoicePCMBuffer.mm)
target_compile_options(voice-session-delivery-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-session-delivery-test PRIVATE apple-client "-framework AVFoundation" "-framework Speech" "-framework CoreAudio" "-framework AudioToolbox")
add_test(NAME voice-session-delivery COMMAND voice-session-delivery-test)
add_executable(shortcut-test tests/input/ShortcutTest.mm src/core/DiagnosticLog.cpp src/core/BoundedFileReader.mm src/core/UpdateController.mm src/settings/AppearancePreferences.mm src/candidate/CandidateSkin.cpp src/candidate/CandidateSkinPreviewView.mm src/candidate/SkinSettingsView.mm src/core/ChineseTextConversion.mm src/settings/ShuangpinKeymapPanel.mm src/input/InputSourceRegistration.mm src/core/FloatingToolbarPanel.mm src/dictionary/DictionaryWindowController.mm src/core/ClientDictionaryRuntime.mm)
target_compile_options(shortcut-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(shortcut-test PRIVATE apple-client "-framework AppKit" "-framework InputMethodKit" "-framework Carbon" "-framework ApplicationServices")
# 控制器撤回大写锁定用的 IOKit；别的控制器测试经 MSIME_CONTROLLER_TEST_LIBS 一并拿到。
target_link_libraries(shortcut-test PRIVATE "-framework IOKit")
target_compile_options(shortcut-test PRIVATE ${MSIME_SPARKLE_COMPILE_OPTIONS})
target_link_libraries(shortcut-test PRIVATE "${MSIME_SPARKLE_FRAMEWORK}")
target_sources(shortcut-test PRIVATE src/settings/PreferencesWindowController.mm src/core/SupportWindowController.mm src/candidate/CandidateSkinAppearance.mm src/core/ScreenKeyboardPanel.mm src/voice/VoiceInputService.mm src/voice/VoicePCMBuffer.mm src/voice/VoiceWaveOverlay.mm src/voice/VoiceCuePlayer.mm src/voice/VoiceAudioMuter.mm src/voice/VoiceSettings.mm src/voice/VoiceProviderSettings.mm src/voice/TextStreamSupport.mm src/voice/OpenLessImport.mm src/cloud/CloudCandidateRequest.mm src/core/CustomTranslationBatch.mm)
target_sources(shortcut-test PRIVATE src/voice/HTTPVoiceRequest.mm src/voice/DoubaoVoiceRequest.mm src/voice/LocalVoiceRequest.mm src/voice/VoiceContextCapture.mm src/voice/LocalPolishServer.mm src/voice/VoicePolishClient.mm src/voice/VoicePipelineHelper.mm src/voice/VoiceEditTracker.mm src/voice/VoiceLearnedToast.mm src/voice/VoiceRecoveryToast.mm src/core/DesktopInputSession.mm)
target_link_libraries(shortcut-test PRIVATE "-framework Security")
target_sources(shortcut-test PRIVATE src/core/DesktopCloudClipboard.mm)
target_link_libraries(shortcut-test PRIVATE msime-voice-providers)
target_link_libraries(shortcut-test PRIVATE "-framework AudioToolbox")
target_link_libraries(MSIMEClientInputMethod PRIVATE "-framework AudioToolbox")
target_link_libraries(shortcut-test PRIVATE "-framework AVFoundation" "-framework Speech" "-framework CoreGraphics" "-framework CoreText" "-framework CoreAudio" "-framework UniformTypeIdentifiers")
add_test(NAME shortcut COMMAND shortcut-test)
add_test(NAME caps-lock COMMAND shortcut-test --caps-lock)
add_test(NAME input-source-restore COMMAND shortcut-test --input-source-restore)
add_test(NAME voice-shortcut-conflicts COMMAND shortcut-test --voice-shortcut-conflicts)
if(MSIME_EDITION STREQUAL "shurufa")
  add_test(NAME shurufa-legacy-save COMMAND shortcut-test --shurufa-legacy-save "$<TARGET_BUNDLE_DIR:MSIMEClientInputMethod>")
  add_test(NAME shurufa-composition COMMAND shortcut-test --shurufa-composition "$<TARGET_BUNDLE_DIR:MSIMEClientInputMethod>")
  add_test(NAME shurufa-system-shortcuts COMMAND shortcut-test --shurufa-system-shortcuts "$<TARGET_BUNDLE_DIR:MSIMEClientInputMethod>")
  add_test(NAME shurufa-system-source COMMAND shortcut-test --shurufa-system-source "$<TARGET_BUNDLE_DIR:MSIMEClientInputMethod>")
  add_test(NAME shurufa-settings-brand COMMAND shortcut-test --shurufa-settings "$<TARGET_BUNDLE_DIR:MSIMEClientInputMethod>")
  file(MAKE_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/shurufa-settings-home")
  set_tests_properties(shurufa-settings-brand shurufa-system-source shurufa-system-shortcuts shurufa-composition shurufa-legacy-save PROPERTIES TIMEOUT 60 ENVIRONMENT "CFFIXED_USER_HOME=${CMAKE_CURRENT_BINARY_DIR}/shurufa-settings-home")
endif()
add_test(NAME shortcut-expanded-scroll COMMAND shortcut-test --expanded-scroll)
add_test(NAME shortcut-vim-mode COMMAND shortcut-test --vim-mode)
add_test(NAME shortcut-nested-commit COMMAND shortcut-test --nested-commit)
add_test(NAME shortcut-english-punctuation COMMAND shortcut-test --english-punctuation)
add_test(NAME shortcut-cross-commit-spacing COMMAND shortcut-test --cross-commit-spacing)
# Around 25 s locally: several cases wait on real input-source and run-loop round trips, which is past the 20 s CI default.
# The controllers under test read runtime-options.json from Application Support, so each run gets an empty home of its own: an installed or demo profile on the developer's Mac must not reach into these cases.
set(MSIME_SHORTCUT_TEST_HOME "${CMAKE_CURRENT_BINARY_DIR}/shortcut-test-home")
file(MAKE_DIRECTORY "${MSIME_SHORTCUT_TEST_HOME}")
set(MSIME_CAPS_LOCK_TEST_HOME "${CMAKE_CURRENT_BINARY_DIR}/caps-lock-test-home")
file(MAKE_DIRECTORY "${MSIME_CAPS_LOCK_TEST_HOME}")
set_tests_properties(caps-lock PROPERTIES TIMEOUT 30 ENVIRONMENT "CFFIXED_USER_HOME=${MSIME_CAPS_LOCK_TEST_HOME}")
set_tests_properties(shortcut-nested-commit PROPERTIES ENVIRONMENT "CFFIXED_USER_HOME=${MSIME_SHORTCUT_TEST_HOME}")
# The ceiling is for the sanitizer leg, where the same binary takes several times as long: measured
# at about 16 s unsanitized and about 120 s under AddressSanitizer on one machine. A ceiling that
# only fits the unsanitized run turns the slow leg into a timeout that says nothing about the code,
# which is the opposite of what this number is here to do. It is still an order below the leg's own
# 300 s ctest ceiling, so a genuinely stuck run is still this test's failure rather than the job's.
set_tests_properties(shortcut PROPERTIES TIMEOUT 240 ENVIRONMENT "CFFIXED_USER_HOME=${MSIME_SHORTCUT_TEST_HOME}")
set_tests_properties(shortcut-expanded-scroll PROPERTIES TIMEOUT 60 ENVIRONMENT "CFFIXED_USER_HOME=${MSIME_SHORTCUT_TEST_HOME}")
add_test(NAME shortcut-translations COMMAND shortcut-test --translations)
set_tests_properties(shortcut-translations PROPERTIES ENVIRONMENT "CFFIXED_USER_HOME=${MSIME_SHORTCUT_TEST_HOME}")
add_test(NAME shortcut-translation-font-size COMMAND shortcut-test --translation-font-size)
set_tests_properties(shortcut-translation-font-size PROPERTIES ENVIRONMENT "CFFIXED_USER_HOME=${MSIME_SHORTCUT_TEST_HOME}")
add_executable(voice-hold-shortcut-test tests/voice/VoiceHoldShortcutTest.mm)
target_compile_options(voice-hold-shortcut-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-hold-shortcut-test PRIVATE "-framework AppKit")
add_test(NAME voice-hold-shortcut COMMAND voice-hold-shortcut-test)
add_executable(configured-voice-shortcut-test tests/voice/ConfiguredVoiceShortcutTest.mm)
target_compile_options(configured-voice-shortcut-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(configured-voice-shortcut-test PRIVATE "-framework AppKit")
add_test(NAME configured-voice-shortcut COMMAND configured-voice-shortcut-test)
add_executable(voice-shortcut-swallow-test tests/voice/VoiceShortcutSwallowTest.mm)
target_compile_options(voice-shortcut-swallow-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-shortcut-swallow-test PRIVATE "-framework AppKit" "-framework ApplicationServices" "-framework CoreGraphics")
add_test(NAME voice-shortcut-swallow COMMAND voice-shortcut-swallow-test)
add_executable(toggle-hotkeys-test tests/input/ToggleHotkeysTest.cpp)
target_compile_options(toggle-hotkeys-test PRIVATE -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(toggle-hotkeys-test PRIVATE apple-client)
add_test(NAME toggle-hotkeys COMMAND toggle-hotkeys-test)

add_executable(toggle-hotkeys-layout-test tests/input/ToggleHotkeysLayoutTest.cpp)
target_compile_options(toggle-hotkeys-layout-test PRIVATE -Wall -Wextra -Werror -UNDEBUG)
add_test(NAME toggle-hotkeys-layout COMMAND toggle-hotkeys-layout-test)

add_executable(toggle-hotkey-event-test tests/input/ToggleHotkeyEventTest.mm)
target_compile_options(toggle-hotkey-event-test PRIVATE -Wall -Wextra -Werror -UNDEBUG -fobjc-arc)
target_link_libraries(toggle-hotkey-event-test PRIVATE "-framework AppKit")
add_test(NAME toggle-hotkey-event COMMAND toggle-hotkey-event-test)
# 重绑路径（改键后新键命中、旧键不命中）单独可跑：全量 shortcut suite 在无图形会话的机器上先倒在皮肤颜色断言。
add_test(NAME shortcut-toggle-hotkeys COMMAND shortcut-test --toggle-hotkeys)
add_executable(editable-shift-focus-test tests/input/EditableShiftFocusTest.mm)
target_compile_options(editable-shift-focus-test PRIVATE -Wall -Wextra -Werror -UNDEBUG -fobjc-arc)
target_link_libraries(editable-shift-focus-test PRIVATE "-framework AppKit" "-framework ApplicationServices")
add_test(NAME editable-shift-focus COMMAND editable-shift-focus-test)
add_test(NAME shortcut-editable-shift COMMAND shortcut-test --editable-shift)
set_tests_properties(shortcut-editable-shift PROPERTIES ENVIRONMENT "CFFIXED_USER_HOME=${MSIME_SHORTCUT_TEST_HOME}")
get_target_property(MSIME_CONTROLLER_TEST_SOURCES shortcut-test SOURCES)
list(REMOVE_ITEM MSIME_CONTROLLER_TEST_SOURCES tests/input/ShortcutTest.mm)
add_executable(http-voice-controller-test tests/voice/HTTPVoiceControllerTest.mm ${MSIME_CONTROLLER_TEST_SOURCES})
target_sources(http-voice-controller-test PRIVATE src/cloud/TranslationCache.mm src/cloud/TranslationSettingsWindow.mm src/core/AISettingsWindow.mm src/input/InputModeHUDPanel.mm src/input/InputModeStatusItem.mm)
get_target_property(MSIME_CONTROLLER_TEST_LIBS shortcut-test LINK_LIBRARIES)
add_executable(korean-romaja-controller-test tests/input/KoreanRomajaControllerTest.mm ${MSIME_CONTROLLER_TEST_SOURCES}
  src/cloud/TranslationCache.mm src/cloud/TranslationSettingsWindow.mm src/core/AISettingsWindow.mm
  src/input/InputModeHUDPanel.mm src/input/InputModeStatusItem.mm)
target_link_libraries(korean-romaja-controller-test PRIVATE ${MSIME_CONTROLLER_TEST_LIBS})
target_compile_options(korean-romaja-controller-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG ${MSIME_SPARKLE_COMPILE_OPTIONS})
add_test(NAME korean-romaja-controller COMMAND korean-romaja-controller-test)
set_tests_properties(korean-romaja-controller PROPERTIES TIMEOUT 60)

# 系统输入验收独立于控制器测试：CI 只编译并探测条件，实际运行需要已安装输入源的桌面会话。
add_executable(korean-system-input-acceptance MACOSX_BUNDLE tests/input/KoreanSystemInputAcceptance.mm)
set_target_properties(korean-system-input-acceptance PROPERTIES
  MACOSX_BUNDLE_GUI_IDENTIFIER "app.msime.tests.KoreanSystemInputAcceptance"
  MACOSX_BUNDLE_BUNDLE_NAME "MSIME Korean System Input Acceptance")
target_compile_options(korean-system-input-acceptance PRIVATE -fobjc-arc -Wall -Wextra -Werror)
target_link_libraries(korean-system-input-acceptance PRIVATE "-framework AppKit" "-framework Carbon")
add_test(NAME korean-system-input-prerequisites COMMAND korean-system-input-acceptance --probe)
target_link_libraries(http-voice-controller-test PRIVATE ${MSIME_CONTROLLER_TEST_LIBS})
target_compile_options(http-voice-controller-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG ${MSIME_SPARKLE_COMPILE_OPTIONS})
add_test(NAME http-voice-controller COMMAND http-voice-controller-test)
add_executable(doubao-voice-controller-test tests/voice/DoubaoVoiceControllerTest.mm ${MSIME_CONTROLLER_TEST_SOURCES} src/cloud/TranslationCache.mm src/cloud/TranslationSettingsWindow.mm src/core/AISettingsWindow.mm src/input/InputModeHUDPanel.mm src/input/InputModeStatusItem.mm)
target_link_libraries(doubao-voice-controller-test PRIVATE ${MSIME_CONTROLLER_TEST_LIBS})
target_compile_options(doubao-voice-controller-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG ${MSIME_SPARKLE_COMPILE_OPTIONS})
add_test(NAME doubao-voice-controller COMMAND doubao-voice-controller-test)
add_executable(live-voice-controller-test tests/voice/LiveVoiceControllerTest.mm ${MSIME_CONTROLLER_TEST_SOURCES} src/cloud/TranslationCache.mm src/cloud/TranslationSettingsWindow.mm src/core/AISettingsWindow.mm src/input/InputModeHUDPanel.mm src/input/InputModeStatusItem.mm)
target_link_libraries(live-voice-controller-test PRIVATE ${MSIME_CONTROLLER_TEST_LIBS})
target_compile_options(live-voice-controller-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG ${MSIME_SPARKLE_COMPILE_OPTIONS})
add_test(NAME live-voice-controller COMMAND live-voice-controller-test)
if(MSIME_EDITION STREQUAL "shurufa")
    add_test(NAME persistent-voice-controller COMMAND live-voice-controller-test --persistent $<TARGET_BUNDLE_DIR:MSIMEClientInputMethod>)
endif()
add_test(NAME live-voice-socket-route COMMAND live-voice-controller-test --socket)
set_tests_properties(live-voice-socket-route PROPERTIES ENVIRONMENT "MSIME_VOICE_PROVIDER_SOCKET=/tmp/synthetic-live.sock")
# The window's implementation is linked in so the test can ask the runtime what properties the class
# actually declares; nothing here opens it, reads a keychain item or writes a default.
# Built from the controller's own sources, like the voice controller tests, so it runs whatever happens to
# the larger shortcut suite.
add_executable(smart-punctuation-space-test tests/input/SmartPunctuationSpaceTest.mm ${MSIME_CONTROLLER_TEST_SOURCES}
  src/cloud/TranslationCache.mm src/cloud/TranslationSettingsWindow.mm src/core/AISettingsWindow.mm
  src/input/InputModeHUDPanel.mm src/input/InputModeStatusItem.mm)
target_link_libraries(smart-punctuation-space-test PRIVATE ${MSIME_CONTROLLER_TEST_LIBS})
target_compile_options(smart-punctuation-space-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG ${MSIME_SPARKLE_COMPILE_OPTIONS})
add_test(NAME smart-punctuation-space COMMAND smart-punctuation-space-test)
set_tests_properties(smart-punctuation-space PROPERTIES TIMEOUT 30)
add_executable(smart-punctuation-shadow-test tests/input/SmartPunctuationShadowTest.mm ${MSIME_CONTROLLER_TEST_SOURCES}
  src/cloud/TranslationCache.mm src/cloud/TranslationSettingsWindow.mm src/core/AISettingsWindow.mm
  src/input/InputModeHUDPanel.mm src/input/InputModeStatusItem.mm)
target_link_libraries(smart-punctuation-shadow-test PRIVATE ${MSIME_CONTROLLER_TEST_LIBS})
target_compile_options(smart-punctuation-shadow-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG ${MSIME_SPARKLE_COMPILE_OPTIONS})
add_test(NAME smart-punctuation-shadow COMMAND smart-punctuation-shadow-test)
set_tests_properties(smart-punctuation-shadow PROPERTIES TIMEOUT 30)
add_executable(cross-commit-spacing-test tests/input/CrossCommitSpacingTest.mm ${MSIME_CONTROLLER_TEST_SOURCES}
  src/cloud/TranslationCache.mm src/cloud/TranslationSettingsWindow.mm src/core/AISettingsWindow.mm
  src/input/InputModeHUDPanel.mm src/input/InputModeStatusItem.mm)
target_link_libraries(cross-commit-spacing-test PRIVATE ${MSIME_CONTROLLER_TEST_LIBS})
target_compile_options(cross-commit-spacing-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG ${MSIME_SPARKLE_COMPILE_OPTIONS})
add_test(NAME cross-commit-spacing COMMAND cross-commit-spacing-test)
set_tests_properties(cross-commit-spacing PROPERTIES TIMEOUT 30)
# Dictionary maintenance from the settings window: real sessions let go of the dictionary lock under a live quiesce lease and come back, mode intact, once it is gone.
add_executable(dictionary-quiesce-controller-test tests/input/DictionaryQuiesceTest.mm ${MSIME_CONTROLLER_TEST_SOURCES}
  src/cloud/TranslationCache.mm src/cloud/TranslationSettingsWindow.mm src/core/AISettingsWindow.mm
  src/input/InputModeHUDPanel.mm src/input/InputModeStatusItem.mm)
target_link_libraries(dictionary-quiesce-controller-test PRIVATE ${MSIME_CONTROLLER_TEST_LIBS})
target_compile_options(dictionary-quiesce-controller-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG ${MSIME_SPARKLE_COMPILE_OPTIONS})
add_test(NAME dictionary-quiesce-controller COMMAND dictionary-quiesce-controller-test)
set_tests_properties(dictionary-quiesce-controller PROPERTIES TIMEOUT 30)
# The lease file format and its raise/lower, shared with the Linux hosts.
add_executable(common-dictionary-quiesce-lease-test ../common/tests/dictionary_quiesce_lease.cpp)
target_compile_features(common-dictionary-quiesce-lease-test PRIVATE cxx_std_17)
target_compile_options(common-dictionary-quiesce-lease-test PRIVATE -Wall -Wextra -Werror -UNDEBUG)
add_test(NAME common-dictionary-quiesce-lease COMMAND common-dictionary-quiesce-lease-test)
add_executable(voice-provider-settings-keys-test tests/voice/VoiceProviderSettingsKeysTest.mm
  src/voice/VoiceProviderSettings.mm src/voice/TextStreamSupport.mm src/voice/OpenLessImport.mm)
target_compile_options(voice-provider-settings-keys-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-provider-settings-keys-test PRIVATE apple-client "-framework AppKit" "-framework Security"
  "-framework CoreAudio" "-framework AudioToolbox")
add_test(NAME voice-provider-settings-keys COMMAND voice-provider-settings-keys-test)
add_executable(voice-provider-catalog-test tests/voice/VoiceProviderCatalogTest.mm
  src/voice/VoiceProviderSettings.mm src/voice/TextStreamSupport.mm src/voice/OpenLessImport.mm)
target_compile_options(voice-provider-catalog-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-provider-catalog-test PRIVATE apple-client "-framework AppKit" "-framework Security"
  "-framework CoreAudio" "-framework AudioToolbox")
add_test(NAME voice-provider-catalog COMMAND voice-provider-catalog-test)
add_executable(voice-provider-settings-validation-test tests/voice/VoiceSettingsTests.mm
  src/voice/VoiceProviderSettings.mm src/voice/TextStreamSupport.mm src/voice/OpenLessImport.mm)
target_compile_options(voice-provider-settings-validation-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-provider-settings-validation-test PRIVATE apple-client "-framework AppKit" "-framework Security"
  "-framework CoreAudio" "-framework AudioToolbox")
add_test(NAME voice-provider-settings-validation COMMAND voice-provider-settings-validation-test)
add_executable(voice-shortcut-recorder-test tests/voice/VoiceShortcutRecorderTest.mm
  src/voice/VoiceProviderSettings.mm src/voice/TextStreamSupport.mm src/voice/OpenLessImport.mm)
target_compile_options(voice-shortcut-recorder-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-shortcut-recorder-test PRIVATE apple-client "-framework AppKit" "-framework Security"
  "-framework CoreAudio" "-framework AudioToolbox")
add_test(NAME voice-shortcut-recorder COMMAND voice-shortcut-recorder-test)
add_executable(voice-polish-presets-test tests/voice/VoicePolishPresetsTest.mm)
target_compile_options(voice-polish-presets-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-polish-presets-test PRIVATE "-framework AppKit")
add_test(NAME voice-polish-presets COMMAND voice-polish-presets-test)
add_executable(voice-polish-request-test tests/voice/VoicePolishRequestTest.mm src/voice/HTTPVoiceRequest.mm)
target_compile_options(voice-polish-request-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-polish-request-test PRIVATE msime-voice-providers "-framework Foundation")
add_test(NAME voice-polish-request COMMAND "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/voice/voice_polish_fixture.py" "$<TARGET_FILE:voice-polish-request-test>")
# The fixture waits out two six-second stalls and gives the run 60 s; without its own limit CI's `ctest --timeout 20` killed it on every run.
set_tests_properties(voice-polish-request PROPERTIES TIMEOUT 65)
# Which prompt each slot resolves to. The request test above sends whatever it is handed; this one says what
# that should be. The header is shared/voice's and this host ships it through HTTPVoiceRequest.mm, but its
# only test ran in the Windows build - so a fallback that had drifted from the reference sat here unchecked.
add_executable(shared-polish-prompt-test ../../shared/voice/tests/polish_prompt.cpp)
target_compile_options(shared-polish-prompt-test PRIVATE -Wall -Wextra -Werror -UNDEBUG)
add_test(NAME shared-polish-prompt COMMAND shared-polish-prompt-test)
target_sources(shortcut-test PRIVATE src/cloud/TranslationCache.mm src/input/InputModeHUDPanel.mm src/input/InputModeStatusItem.mm)
add_executable(shuangpin-keymap-input-test tests/input/ShuangpinKeymapInputTest.mm src/settings/ShuangpinKeymapPanel.mm)
target_include_directories(shuangpin-keymap-input-test PRIVATE src)
target_compile_options(shuangpin-keymap-input-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(shuangpin-keymap-input-test PRIVATE apple-client "-framework AppKit")
add_test(NAME shuangpin-keymap-input COMMAND shuangpin-keymap-input-test)
add_executable(candidate-panel-test tests/candidate/CandidatePanelTests.mm src/candidate/CandidatePanel.mm src/candidate/CandidateMotion.mm src/core/DiagnosticLog.cpp src/candidate/CandidateSkinAppearance.mm src/candidate/CandidateSkin.cpp)
target_compile_options(candidate-panel-test PRIVATE -fobjc-arc -Wall -Wextra -Werror)
target_link_libraries(candidate-panel-test PRIVATE apple-client "-framework AppKit" "-framework InputMethodKit" "-framework QuartzCore" "-framework ImageIO")
add_test(NAME candidate-panel COMMAND candidate-panel-test)
# 候选释义的词性显示与单条释义两个开关，纯逻辑，不依赖窗口。
add_executable(candidate-translation-display-test tests/core/CandidateTranslationDisplayTest.cpp)
target_compile_options(candidate-translation-display-test PRIVATE -Wall -Wextra -Werror -UNDEBUG)
add_test(NAME candidate-translation-display COMMAND candidate-translation-display-test)
# 候选徽标的字形选择（AI 来源两档、固定排位开关与三档样式），纯逻辑，不依赖窗口。
add_executable(candidate-badge-policy-test tests/core/CandidateBadgePolicyTest.cpp)
target_compile_options(candidate-badge-policy-test PRIVATE -Wall -Wextra -Werror -UNDEBUG)
add_test(NAME candidate-badge-policy COMMAND candidate-badge-policy-test)
# The readings drawn after gloss lines: language per line, romaji from the system tokenizer, and the drawn string.
add_executable(candidate-pronunciation-test tests/candidate/CandidatePronunciationTest.mm)
target_compile_options(candidate-pronunciation-test PRIVATE -fobjc-arc -UNDEBUG -Wall -Wextra -Werror)
target_link_libraries(candidate-pronunciation-test PRIVATE "-framework Foundation")
add_test(NAME candidate-pronunciation COMMAND candidate-pronunciation-test)
# The Windows candidate_item_layout rule, ported: text, 辅助码 and glosses wider than their column wrap inside it and rows take their own heights.
add_executable(candidate-item-layout-test tests/candidate/CandidateItemLayoutTest.cpp)
target_compile_options(candidate-item-layout-test PRIVATE -Wall -Wextra -Werror -UNDEBUG)
add_test(NAME candidate-item-layout COMMAND candidate-item-layout-test)
# The candidate page layout rendered through the same controller shortcut-test imports, on its own so its assertions report independently of that suite.
add_executable(candidate-row-layout-test tests/candidate/CandidateRowLayoutTest.mm ${MSIME_CONTROLLER_TEST_SOURCES} src/cloud/TranslationCache.mm src/cloud/TranslationSettingsWindow.mm src/core/AISettingsWindow.mm src/input/InputModeHUDPanel.mm src/input/InputModeStatusItem.mm)
target_link_libraries(candidate-row-layout-test PRIVATE ${MSIME_CONTROLLER_TEST_LIBS})
target_compile_options(candidate-row-layout-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG ${MSIME_SPARKLE_COMPILE_OPTIONS})
add_test(NAME candidate-row-layout COMMAND candidate-row-layout-test)
add_test(NAME candidate-user-geometry COMMAND candidate-row-layout-test --user-geometry)
add_test(NAME candidate-host-appearance COMMAND candidate-row-layout-test --host-appearance)
# 光标后编辑：视图字段、首行右侧的提示和首选开头画淡。
add_test(NAME candidate-caret-edit-suffix COMMAND candidate-row-layout-test --caret-edit-suffix)
# 计时动效独立启动事件循环，不承接大排版矩阵的窗口与图层回调。
add_test(NAME candidate-declared-motion COMMAND candidate-row-layout-test --declared-motion)
# 大皮肤矩阵各自运行，保留 CI 的单项时限和全部排版断言。
add_test(NAME candidate-factory-layout COMMAND candidate-row-layout-test --factory-layout)
add_test(NAME candidate-factory-shadow COMMAND candidate-row-layout-test --shadow-host)
add_test(NAME candidate-factory-pager COMMAND candidate-row-layout-test --pager-host)
add_test(NAME candidate-factory-expand-toggle COMMAND candidate-row-layout-test --expand-toggle)
# 装饰词的真实控制器绘制独立运行，避免大布局矩阵与动效计时互相影响。
add_executable(candidate-decoration-overlay-test tests/candidate/CandidateDecorationOverlayTest.mm ${MSIME_CONTROLLER_TEST_SOURCES} src/cloud/TranslationCache.mm src/cloud/TranslationSettingsWindow.mm src/core/AISettingsWindow.mm src/input/InputModeHUDPanel.mm src/input/InputModeStatusItem.mm)
target_link_libraries(candidate-decoration-overlay-test PRIVATE ${MSIME_CONTROLLER_TEST_LIBS})
target_compile_options(candidate-decoration-overlay-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG ${MSIME_SPARKLE_COMPILE_OPTIONS})
add_test(NAME candidate-decoration-overlay COMMAND candidate-decoration-overlay-test)
set_tests_properties(candidate-decoration-overlay PROPERTIES TIMEOUT 30)
# Canvas 的切片与动画资源独立验证，不依赖输入会话和词库。
add_executable(candidate-canvas-test tests/candidate/CandidateCanvasTest.mm src/candidate/CandidateCanvas.mm)
target_compile_options(candidate-canvas-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(candidate-canvas-test PRIVATE "-framework AppKit" "-framework QuartzCore" "-framework ImageIO")
add_test(NAME candidate-canvas COMMAND candidate-canvas-test)
add_executable(candidate-hover-test tests/candidate/CandidateHoverTest.mm src/candidate/CandidateCanvas.mm)
target_compile_options(candidate-hover-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(candidate-hover-test PRIVATE "-framework AppKit" "-framework QuartzCore" "-framework ImageIO")
add_test(NAME candidate-hover COMMAND candidate-hover-test)
# 候选右键菜单画成不能成为 key 的非激活面板：窗口属性、子菜单、点按派发和贴边翻转。
add_executable(candidate-pointer-menu-test tests/candidate/CandidatePointerMenuTest.mm)
target_compile_options(candidate-pointer-menu-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(candidate-pointer-menu-test PRIVATE "-framework AppKit")
add_test(NAME candidate-pointer-menu COMMAND candidate-pointer-menu-test)
# Where the candidate window goes: under the caret, flipped above it when there is no room, and
# clamped into the display it is on - including one whose coordinates are negative.
add_executable(candidate-placement-test tests/candidate/CandidatePlacementTest.mm)
target_compile_options(candidate-placement-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(candidate-placement-test PRIVATE "-framework AppKit")
add_test(NAME candidate-placement COMMAND candidate-placement-test)
add_executable(candidate-pagination-test tests/candidate/CandidatePaginationTest.cpp)
target_include_directories(candidate-pagination-test PRIVATE src)
target_compile_options(candidate-pagination-test PRIVATE -Wall -Wextra -Werror -UNDEBUG)
add_test(NAME candidate-pagination COMMAND candidate-pagination-test)
# 展开的候选网格（#7307）里光标怎么走、可见的是哪几行。
add_executable(candidate-expanded-grid-test tests/candidate/CandidateExpandedGridTest.cpp)
target_include_directories(candidate-expanded-grid-test PRIVATE src)
target_compile_options(candidate-expanded-grid-test PRIVATE -Wall -Wextra -Werror -UNDEBUG)
add_test(NAME candidate-expanded-grid COMMAND candidate-expanded-grid-test)
# src/candidate/CandidatePanel.mm is the panel of the Apple snapshot's direct-engine controller, which is gone; only candidate-panel-test builds it.
# Installing a dictionary over the one already in place. Every refusal here protects that file, and
# the install itself must not be able to leave the user without one.
add_executable(dictionary-installer-test tests/dictionary/DictionaryInstallerTest.mm src/dictionary/DictionaryInstaller.mm)
target_compile_options(dictionary-installer-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(dictionary-installer-test PRIVATE "-framework Foundation" ${SQLITE_TARGET})
add_test(NAME dictionary-installer COMMAND dictionary-installer-test)
add_executable(dictionary-runtime-test tests/dictionary/DictionaryRuntimeTest.mm src/core/ClientDictionaryRuntime.mm)
target_compile_options(dictionary-runtime-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_include_directories(dictionary-runtime-test PRIVATE ../../shared/apple ../../crates/host-api/include)
target_link_libraries(dictionary-runtime-test PRIVATE apple-client "-framework Foundation")
add_test(NAME dictionary-runtime COMMAND dictionary-runtime-test)
add_executable(skin-preview-test tests/candidate/SkinPreviewTest.mm src/settings/AppearancePreferences.mm src/core/BoundedFileReader.mm src/candidate/CandidateSkin.cpp src/candidate/CandidateSkinPreviewView.mm src/candidate/SkinSettingsView.mm src/core/ChineseTextConversion.mm src/settings/PreferencesWindowController.mm src/candidate/CandidateSkinAppearance.mm)
target_include_directories(skin-preview-test PRIVATE src)
target_compile_options(skin-preview-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(skin-preview-test PRIVATE "-framework AppKit" "-framework CoreText")
add_test(NAME skin-preview COMMAND skin-preview-test)
# 252 组主题排版与绘制保留全部组合，各主题独立运行以沿用单项 20 秒预算。
foreach(skin system shuishan light paper night ink)
  add_test(NAME skin-preview-${skin} COMMAND skin-preview-test --theme-matrix ${skin})
endforeach()
add_executable(skin-settings-test tests/candidate/SkinSettingsTest.mm src/settings/AppearancePreferences.mm src/core/BoundedFileReader.mm src/candidate/CandidateSkin.cpp src/candidate/CandidateSkinPreviewView.mm src/candidate/SkinSettingsView.mm src/candidate/CandidateSkinAppearance.mm src/core/ChineseTextConversion.mm)
foreach(translation_settings_consumer MSIMEClientInputMethod toolbar-visibility-preferences-test scheme-round-trip-preferences-test wubi-auto-commit-preferences-test local-mode-preferences-test candidate-appearance-defaults-test voice-hold-defaults-test skin-package-replace-test preferences-lifecycle-test shortcut-test skin-preview-test skin-settings-test)
  target_sources(${translation_settings_consumer} PRIVATE src/cloud/TranslationSettingsWindow.mm)
endforeach()
foreach(ai_settings_consumer toolbar-visibility-preferences-test scheme-round-trip-preferences-test wubi-auto-commit-preferences-test local-mode-preferences-test candidate-appearance-defaults-test voice-hold-defaults-test skin-package-replace-test preferences-lifecycle-test shortcut-test skin-preview-test skin-settings-test)
  target_sources(${ai_settings_consumer} PRIVATE src/core/AISettingsWindow.mm)
endforeach()
foreach(translation_settings_consumer toolbar-visibility-preferences-test scheme-round-trip-preferences-test skin-preview-test skin-settings-test)
  target_link_libraries(${translation_settings_consumer} PRIVATE apple-client)
endforeach()
target_include_directories(skin-settings-test PRIVATE src)
target_compile_options(skin-settings-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(skin-settings-test PRIVATE "-framework AppKit")
add_test(NAME skin-settings COMMAND skin-settings-test)
add_executable(floating-toolbar-panel-test tests/settings/FloatingToolbarPanelTest.mm src/core/FloatingToolbarPanel.mm src/core/SupportWindowController.mm src/candidate/CandidateSkinAppearance.mm src/candidate/CandidateSkin.cpp)
target_compile_options(floating-toolbar-panel-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(floating-toolbar-panel-test PRIVATE "-framework AppKit")
add_test(NAME floating-toolbar-panel COMMAND floating-toolbar-panel-test)
add_executable(window-presentation-test tests/core/WindowPresentationTest.mm)
target_compile_options(window-presentation-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(window-presentation-test PRIVATE "-framework AppKit")
add_test(NAME window-presentation COMMAND window-presentation-test)
add_executable(settings-edit-menu-test tests/core/SettingsEditMenuTest.mm)
target_compile_options(settings-edit-menu-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(settings-edit-menu-test PRIVATE "-framework AppKit")
add_test(NAME settings-edit-menu COMMAND settings-edit-menu-test)
# The Sparkle driver is compiled in, but the test binary is not an application bundle, so the shared
# controller must decline to start it - which is the thing worth testing.
add_executable(update-controller-test tests/core/UpdateControllerTest.mm src/core/UpdateController.mm)
target_compile_options(update-controller-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(update-controller-test PRIVATE "-framework AppKit")
if(MSIME_ENABLE_SPARKLE)
  target_compile_options(update-controller-test PRIVATE ${MSIME_SPARKLE_COMPILE_OPTIONS})
  target_link_libraries(update-controller-test PRIVATE "${MSIME_SPARKLE_FRAMEWORK}")
endif()
add_test(NAME update-controller COMMAND update-controller-test)
set_tests_properties(update-controller PROPERTIES TIMEOUT 20)
add_executable(installed-host-test tests/core/InstalledHostTest.mm)
target_compile_options(installed-host-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(installed-host-test PRIVATE "-framework Foundation")
add_test(NAME installed-host COMMAND installed-host-test)
add_executable(shurufa-preference-migration-test tests/settings/ShurufaPreferenceMigrationTest.mm)
target_compile_options(shurufa-preference-migration-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(shurufa-preference-migration-test PRIVATE "-framework Foundation")
add_test(NAME shurufa-preference-migration COMMAND shurufa-preference-migration-test)
add_test(NAME shurufa-installation COMMAND "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/core/shurufa_installation_test.py")
add_test(NAME shurufa-brand-audit COMMAND "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tests/core/shurufa_brand_audit_test.py")
add_executable(support-window-test tests/core/SupportWindowTest.mm src/core/SupportWindowController.mm)
target_compile_options(support-window-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(support-window-test PRIVATE "-framework AppKit")
add_test(NAME support-window COMMAND support-window-test)
add_test(
  NAME support-window-links
  COMMAND "${Python3_EXECUTABLE}"
          "${CMAKE_CURRENT_SOURCE_DIR}/tests/settings/support_window_links.py"
          "${CMAKE_CURRENT_SOURCE_DIR}/src/core/SupportWindowController.mm")
# The template checks say what the build was told to produce; this one says what came out. A resource can
# be declared and not staged, a localisation can exist in the tree and not be copied, and the on-device
# recogniser can be compiled into a library the executable never links - each invisible until an install.
# The settings page is shared, so a preference added for another platform appears on macOS too. When the
# host does not read it the switch saves, the page says it saved, and nothing changes - which is how 本地
# Whisper and the smart-punctuation direct switches sat there doing nothing. Nothing else catches that,
# because the host compiles and the tests pass either way.
# The settings page list lives in TypeScript and the route vocabulary in Rust, so nothing connects them.
# A page added without a category is a page no host can open - which is how the account page ended up with
# macOS showing its own window instead of the one every other platform shows.
add_test(
  NAME settings-route-coverage
  COMMAND "${Python3_EXECUTABLE}"
          "${CMAKE_CURRENT_SOURCE_DIR}/tests/settings/settings_route_coverage.py"
          "${CMAKE_CURRENT_SOURCE_DIR}/../..")
add_test(
  NAME preference-coverage
  COMMAND "${Python3_EXECUTABLE}"
          "${CMAKE_CURRENT_SOURCE_DIR}/tests/settings/preference_coverage.py"
          "${CMAKE_CURRENT_SOURCE_DIR}/../..")
# 官方插件包随这次构建放进了 bundle 时，bundle-contents 逐个核对它们到位、内容和构建出的一致。不内置语音运行库的构建，它核对运行库确实不在 bundle 里。
set(MSIME_BUNDLE_VOICE_RUNTIME_ARGUMENTS "")
if(NOT MSIME_BUNDLE_VOICE_RUNTIME)
  set(MSIME_BUNDLE_VOICE_RUNTIME_ARGUMENTS --without-voice-runtime)
endif()
set(MSIME_BUNDLE_PLUGIN_PACKS_ARGUMENTS "")
if(MSIME_STAGED_PLUGIN_PACKS)
  set(MSIME_BUNDLE_PLUGIN_PACKS_ARGUMENTS --plugin-packs "${MSIME_PLUGIN_PACKS_DIR}")
endif()
add_test(
  NAME bundle-contents
  COMMAND "${Python3_EXECUTABLE}"
          "${CMAKE_CURRENT_SOURCE_DIR}/tests/settings/bundle_contents.py"
          "$<TARGET_BUNDLE_DIR:MSIMEClientInputMethod>"
          --languages zh-Hans,en
          ${MSIME_BUNDLE_PLUGIN_PACKS_ARGUMENTS}
          ${MSIME_BUNDLE_VOICE_RUNTIME_ARGUMENTS})
add_test(
  NAME info-plist-icons
  COMMAND "${Python3_EXECUTABLE}"
          "${CMAKE_CURRENT_SOURCE_DIR}/tests/settings/info_plist_icons.py"
          "${MSIME_INFO_PLIST_TEMPLATE}"
          "${CMAKE_CURRENT_SOURCE_DIR}/resources")
add_test(
  NAME info-plist-usage
  COMMAND "${Python3_EXECUTABLE}"
          "${CMAKE_CURRENT_SOURCE_DIR}/tests/settings/info_plist_usage.py"
          "${MSIME_INFO_PLIST_TEMPLATE}"
          "${CMAKE_CURRENT_SOURCE_DIR}/src")
# The input source identifiers and the names the input menu shows for them live in different files, so renaming the bundle leaves the names keyed on an identifier nothing declares any more - and the menu then prints the identifier itself.
add_test(
  NAME info-plist-names
  COMMAND "${Python3_EXECUTABLE}"
          "${CMAKE_CURRENT_SOURCE_DIR}/tests/settings/info_plist_names.py"
          "${MSIME_INFO_PLIST_TEMPLATE}"
          "${MSIME_LOCALIZED_RESOURCES}"
          "${CMAKE_CURRENT_SOURCE_DIR}/../..")
add_test(
  NAME entitlements-guard
  COMMAND "${Python3_EXECUTABLE}"
          "${CMAKE_CURRENT_SOURCE_DIR}/tests/settings/entitlements_guard.py"
          "${CMAKE_CURRENT_SOURCE_DIR}/resources/VoiceInput.entitlements"
          "${CMAKE_CURRENT_SOURCE_DIR}/scripts/install.sh")
add_executable(input-source-registration-test tests/input/InputSourceRegistrationTests.mm src/input/InputSourceRegistration.mm)
target_compile_options(input-source-registration-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(input-source-registration-test PRIVATE "-framework AppKit" "-framework Carbon" "-framework Foundation")
add_test(NAME input-source-registration COMMAND input-source-registration-test)
add_executable(shurufa-input-source-refresh-test tests/input/ShurufaInputSourceRefreshTest.mm src/input/InputSourceRegistration.mm)
target_compile_options(shurufa-input-source-refresh-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(shurufa-input-source-refresh-test PRIVATE "-framework AppKit" "-framework Carbon" "-framework Foundation")
add_test(NAME shurufa-input-source-refresh COMMAND shurufa-input-source-refresh-test)

# The page-size rules had a test file and no target: it had never been built, let alone run, which is
# how a clamp that rewrote the shared default stayed in place.
add_executable(candidate-page-size-test tests/candidate/CandidatePageSizeTest.cpp)
target_compile_options(candidate-page-size-test PRIVATE -Wall -Wextra -Werror -UNDEBUG)
add_test(NAME candidate-page-size COMMAND candidate-page-size-test)

add_executable(voice-wave-overlay-test tests/voice/VoiceWaveOverlayTest.mm src/voice/VoiceWaveOverlay.mm)
target_compile_options(voice-wave-overlay-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-wave-overlay-test PRIVATE "-framework Cocoa")
add_test(NAME voice-wave-overlay COMMAND voice-wave-overlay-test)

add_executable(input-menu-test tests/input/InputMenuTests.mm)
target_compile_options(input-menu-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(input-menu-test PRIVATE "-framework Cocoa")
add_test(NAME input-menu COMMAND input-menu-test)

add_executable(voice-audio-muter-test tests/voice/VoiceAudioMuterTest.mm src/voice/VoiceAudioMuter.mm)
target_compile_options(voice-audio-muter-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-audio-muter-test PRIVATE "-framework Foundation" "-framework CoreAudio")
add_test(NAME voice-audio-muter COMMAND voice-audio-muter-test)

add_executable(voice-cue-player-test tests/voice/VoiceCuePlayerTest.mm src/voice/VoiceCuePlayer.mm)
target_compile_options(voice-cue-player-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_compile_definitions(voice-cue-player-test PRIVATE MSIME_VOICE_CUE_SOURCE_DIR="${CMAKE_CURRENT_SOURCE_DIR}/resources/audios")
target_link_libraries(voice-cue-player-test PRIVATE "-framework AppKit" "-framework AVFoundation" "-framework CoreAudio")
add_test(NAME voice-cue-player COMMAND voice-cue-player-test)
# 播放会卡在无人应答的麦克风授权框上时，测试跑完加载断言后以 77 退出，CTest 记为跳过而不是挂到超时。
set_tests_properties(voice-cue-player PROPERTIES SKIP_RETURN_CODE 77)

add_executable(voice-input-level-test tests/voice/VoiceInputLevelTest.mm)
target_compile_options(voice-input-level-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-input-level-test PRIVATE "-framework Foundation" "-framework AVFoundation")
add_test(NAME voice-input-level COMMAND voice-input-level-test)

add_executable(voice-mute-recovery-test tests/voice/VoiceMuteRecoveryTest.mm src/voice/VoiceAudioMuter.mm)
target_compile_options(voice-mute-recovery-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-mute-recovery-test PRIVATE "-framework Foundation" "-framework CoreAudio")
add_test(NAME voice-mute-recovery COMMAND voice-mute-recovery-test)

target_sources(MSIMEClientInputMethod PRIVATE src/voice/VoiceAudioMuter.mm)
target_link_libraries(MSIMEClientInputMethod PRIVATE "-framework CoreAudio")

add_executable(paired-punctuation-test tests/input/PairedPunctuationTest.cpp)
target_compile_features(paired-punctuation-test PRIVATE cxx_std_17)
target_compile_options(paired-punctuation-test PRIVATE -Wall -Wextra -Werror -UNDEBUG)
add_test(NAME paired-punctuation COMMAND paired-punctuation-test)

add_executable(input-scheme-traits-test tests/input/InputSchemeTraitsTest.cpp)
target_compile_features(input-scheme-traits-test PRIVATE cxx_std_17)
target_compile_options(input-scheme-traits-test PRIVATE -Wall -Wextra -Werror -UNDEBUG)
add_test(NAME input-scheme-traits COMMAND input-scheme-traits-test)

add_executable(candidate-font-cache-test tests/candidate/CandidateFontCacheTest.mm src/candidate/CandidateFontCache.mm)
target_compile_options(candidate-font-cache-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(candidate-font-cache-test PRIVATE "-framework AppKit" "-framework CoreText")
add_test(NAME candidate-font-cache COMMAND candidate-font-cache-test)

# AppearancePreferences.mm resolves candidate font families through CandidateFontCache.mm, so every target compiling the one compiles the other; a loop rather than a hand-kept list so a new consumer cannot miss it.
get_property(MSIME_FONT_CACHE_TARGETS DIRECTORY PROPERTY BUILDSYSTEM_TARGETS)
foreach(msime_target IN LISTS MSIME_FONT_CACHE_TARGETS)
  get_target_property(msime_target_sources ${msime_target} SOURCES)
  if(NOT msime_target_sources OR NOT "src/settings/AppearancePreferences.mm" IN_LIST msime_target_sources)
    continue()
  endif()
  target_sources(${msime_target} PRIVATE src/candidate/CandidateFontCache.mm)
  target_link_libraries(${msime_target} PRIVATE "-framework CoreText")
endforeach()

# AppearancePreferences.mm and InputController.mm reach the Swift account pane and the native uninstaller through weak_import: the input method binds them from MSIMEBackend.dylib and from uninstaller.mm, and a process that has neither checks for null before calling. Test targets are that process, so the linker has to be told those names may stay undefined - one name at a time, so every other missing symbol is still an error.
#
# The settings window's dictionary controller is the counter-example and the reason this loop exists rather than a hand-kept list: `feat(macos): open dictionary manager from settings` added the reference and the source file to shortcut-test only, and the other five targets compiling the same file did not link again until now.
set(MSIME_OPTIONAL_BACKEND_SYMBOLS
  _msime_macos_uninstall_input_source)
# Rust 宿主的 staticlib 由 CMake 链进输入法，Cargo 自己的 `-dead_strip` 管不到这里；不加它的话，ld64 会保留任何一个导出牵连到的所有归档成员（image、rink、rustls 等输入法从不调用的库）。只在 Release 加：链接实验里 arm64 可执行文件从 43 MB 降到 27 MB。Objective-C 类和 category 不受影响，因为 ld64 把 `__objc_classlist` 当作根。
target_link_options(MSIMEClientInputMethod PRIVATE "$<$<CONFIG:Release>:LINKER:-dead_strip>")
get_property(MSIME_DIRECTORY_TARGETS DIRECTORY PROPERTY BUILDSYSTEM_TARGETS)
foreach(msime_target IN LISTS MSIME_DIRECTORY_TARGETS)
  if(msime_target STREQUAL "MSIMEClientInputMethod")
    continue()
  endif()
  get_target_property(msime_target_sources ${msime_target} SOURCES)
  if(NOT msime_target_sources)
    continue()
  endif()
  if(NOT "src/settings/AppearancePreferences.mm" IN_LIST msime_target_sources
     AND NOT "src/input/InputController.mm" IN_LIST msime_target_sources)
    continue()
  endif()
  if(NOT "src/dictionary/DictionaryWindowController.mm" IN_LIST msime_target_sources)
    target_sources(${msime_target} PRIVATE
      src/dictionary/DictionaryWindowController.mm src/core/ClientDictionaryRuntime.mm)
    target_link_libraries(${msime_target} PRIVATE apple-client "-framework UniformTypeIdentifiers")
  endif()
  foreach(msime_optional_symbol IN LISTS MSIME_OPTIONAL_BACKEND_SYMBOLS)
    target_link_options(${msime_target} PRIVATE "LINKER:-U,${msime_optional_symbol}")
  endforeach()
endforeach()

# CandidateSkin.cpp 通过 SkinManifestBridge.mm 读取外部皮肤清单，后者调用 Rust 宿主库里共享的 client-services 加载器。编译其中一个文件的每个目标都需要另一个文件和 apple-client，所以由这个循环把它们绑在一起，而不是手工维护一份新使用方可能漏掉的列表。
set_source_files_properties(src/candidate/SkinManifestBridge.mm src/candidate/CandidateCanvas.mm PROPERTIES COMPILE_OPTIONS "-fobjc-arc")
get_property(MSIME_SKIN_TARGETS DIRECTORY PROPERTY BUILDSYSTEM_TARGETS)
foreach(msime_target IN LISTS MSIME_SKIN_TARGETS)
  get_target_property(msime_target_sources ${msime_target} SOURCES)
  if(NOT msime_target_sources OR NOT "src/candidate/CandidateSkin.cpp" IN_LIST msime_target_sources)
    continue()
  endif()
  if(NOT "src/candidate/SkinManifestBridge.mm" IN_LIST msime_target_sources)
    target_sources(${msime_target} PRIVATE src/candidate/SkinManifestBridge.mm)
  endif()
  target_sources(${msime_target} PRIVATE src/candidate/CandidateCanvas.mm)
  target_link_libraries(${msime_target} PRIVATE apple-client "-framework QuartzCore" "-framework ImageIO")
endforeach()

# 编译悬浮工具栏的所有目标都需要图片画布与共享解码桥。
get_property(MSIME_TOOLBAR_TARGETS DIRECTORY PROPERTY BUILDSYSTEM_TARGETS)
foreach(msime_target IN LISTS MSIME_TOOLBAR_TARGETS)
  get_target_property(msime_target_sources ${msime_target} SOURCES)
  if(msime_target_sources AND "src/core/FloatingToolbarPanel.mm" IN_LIST msime_target_sources)
    target_sources(${msime_target} PRIVATE src/core/ToolbarImageCanvas.mm)
    target_link_libraries(${msime_target} PRIVATE apple-client "-framework QuartzCore")
  endif()
endforeach()

# VoiceInputService.mm passes the system recogniser's text through shared/voice's tidy_local_transcript, so every target compiling it links the library that defines it. Found by source rather than listed, for the same reason as the loops above.
get_property(MSIME_VOICE_SERVICE_TARGETS DIRECTORY PROPERTY BUILDSYSTEM_TARGETS)
foreach(msime_target IN LISTS MSIME_VOICE_SERVICE_TARGETS)
  get_target_property(msime_target_sources ${msime_target} SOURCES)
  if(msime_target_sources AND "src/voice/VoiceInputService.mm" IN_LIST msime_target_sources)
    target_link_libraries(${msime_target} PRIVATE msime-voice-local-asr)
  endif()
endforeach()

# InputController.mm draws typing effects through TypingEffectPanel.mm and candidate skin motion through CandidateMotion.mm, both Core Animation. Every target compiling the controller next to its HUD panel - the input method and each controller test - takes both and QuartzCore, found by source like the loops above so a new controller test cannot miss it.
get_property(MSIME_TYPING_EFFECT_TARGETS DIRECTORY PROPERTY BUILDSYSTEM_TARGETS)
foreach(msime_target IN LISTS MSIME_TYPING_EFFECT_TARGETS)
  get_target_property(msime_target_sources ${msime_target} SOURCES)
  if(NOT msime_target_sources OR NOT "src/input/InputModeHUDPanel.mm" IN_LIST msime_target_sources)
    continue()
  endif()
  if(NOT "src/input/InputController.mm" IN_LIST msime_target_sources
     AND NOT "src/core/FloatingToolbarPanel.mm" IN_LIST msime_target_sources)
    continue()
  endif()
  target_sources(${msime_target} PRIVATE src/input/TypingEffectPanel.mm src/candidate/CandidateMotion.mm)
  # CandidateMotion.mm 用 ImageIO 读 APNG 装饰图的各帧。
  target_link_libraries(${msime_target} PRIVATE "-framework QuartzCore" "-framework ImageIO")
endforeach()

# The typing effect panel on its own: the packed answer decoded, the Reduce Motion and Low Power fallback, and the idle state that leaves no emitter running and no window on screen.
add_executable(typing-effect-panel-test tests/input/TypingEffectPanelTest.mm src/input/TypingEffectPanel.mm)
target_compile_options(typing-effect-panel-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(typing-effect-panel-test PRIVATE "-framework AppKit" "-framework QuartzCore")
add_test(NAME typing-effect-panel COMMAND typing-effect-panel-test)

# Wave 2 本地语音管线单元测试：音频前端预处理、安全上下文采集、本机 llama-server 托管与独立润色客户端。
add_executable(voice-audio-preprocess-test tests/voice/VoiceAudioPreprocessTest.mm)
target_compile_options(voice-audio-preprocess-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
add_test(NAME voice-audio-preprocess COMMAND voice-audio-preprocess-test)

add_executable(voice-context-capture-test tests/voice/VoiceContextCaptureTest.mm src/voice/VoiceContextCapture.mm)
target_compile_options(voice-context-capture-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-context-capture-test PRIVATE "-framework AppKit" "-framework ApplicationServices" "-framework Carbon")
add_test(NAME voice-context-capture COMMAND voice-context-capture-test)

add_executable(local-polish-server-test tests/voice/LocalPolishServerTest.mm src/voice/LocalPolishServer.mm)
target_compile_options(local-polish-server-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(local-polish-server-test PRIVATE "-framework Foundation" "-framework Security")
add_test(NAME local-polish-server COMMAND local-polish-server-test)
set_tests_properties(local-polish-server PROPERTIES TIMEOUT 60)

add_executable(voice-polish-client-test tests/voice/VoicePolishClientTest.mm src/voice/VoicePolishClient.mm src/voice/LocalPolishServer.mm)
target_compile_options(voice-polish-client-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-polish-client-test PRIVATE apple-client "-framework Foundation" "-framework Security")
add_test(NAME voice-polish-client COMMAND voice-polish-client-test)
set_tests_properties(voice-polish-client PROPERTIES TIMEOUT 60)

add_executable(voice-edit-tracker-test tests/voice/VoiceEditTrackerTest.mm src/voice/VoiceEditTracker.mm src/voice/VoiceLearnedToast.mm src/voice/VoiceContextCapture.mm)
target_compile_options(voice-edit-tracker-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-edit-tracker-test PRIVATE apple-client "-framework AppKit" "-framework ApplicationServices" "-framework Carbon")
add_test(NAME voice-edit-tracker COMMAND voice-edit-tracker-test)
set_tests_properties(voice-edit-tracker PROPERTIES TIMEOUT 30)

add_executable(voice-resilience-test tests/voice/VoiceResilienceTest.mm src/voice/VoiceInputService.mm src/voice/VoicePCMBuffer.mm src/voice/VoiceRecoveryToast.mm src/voice/VoiceContextCapture.mm)
target_compile_options(voice-resilience-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
target_link_libraries(voice-resilience-test PRIVATE msime-voice-local-asr "-framework AppKit" "-framework AVFoundation" "-framework CoreAudio" "-framework AudioToolbox" "-framework Speech" "-framework ApplicationServices" "-framework Carbon")
add_test(NAME voice-resilience COMMAND voice-resilience-test)
set_tests_properties(voice-resilience PROPERTIES TIMEOUT 30)

# Shurufa v2 Tier 1 端侧神经解码共享库（libllama Apple Metal）与微基准测试。
# 仅在本机检测到 `llama.h` 与 `libllama` 时启用，缺失依赖的机器自动跳过，不影响纯 Tier 0 构建。
find_path(SHURUFA_LLAMA_INCLUDE_DIR llama.h
  HINTS "${CMAKE_PREFIX_PATH}/include" /opt/homebrew/include /usr/local/include)
find_library(SHURUFA_LLAMA_LIBRARY NAMES llama
  HINTS "${CMAKE_PREFIX_PATH}/lib" /opt/homebrew/lib /usr/local/lib)
if(SHURUFA_LLAMA_INCLUDE_DIR AND SHURUFA_LLAMA_LIBRARY)
  get_filename_component(SHURUFA_LLAMA_LIB_DIR "${SHURUFA_LLAMA_LIBRARY}" DIRECTORY)
  add_library(shurufa_neural SHARED src/neural/ShurufaNeuralEngine.cpp)
  target_compile_features(shurufa_neural PRIVATE cxx_std_17)
  target_compile_options(shurufa_neural PRIVATE -Wall -Wextra -Werror -fvisibility=hidden)
  target_include_directories(shurufa_neural
    PUBLIC "${CMAKE_CURRENT_SOURCE_DIR}/src/neural"
    PRIVATE "${SHURUFA_LLAMA_INCLUDE_DIR}")
  target_link_libraries(shurufa_neural PRIVATE "${SHURUFA_LLAMA_LIBRARY}" ${SQLITE_TARGET} "-framework CoreFoundation")
  set_target_properties(shurufa_neural PROPERTIES
    OUTPUT_NAME "shurufa_neural"
    INSTALL_NAME_DIR "@rpath"
    BUILD_RPATH "@loader_path;${SHURUFA_LLAMA_LIB_DIR}")

  add_executable(shurufa-neural-bench src/neural/NeuralEngineBench.cpp)
  target_compile_features(shurufa-neural-bench PRIVATE cxx_std_17)
  target_compile_options(shurufa-neural-bench PRIVATE -Wall -Wextra -Werror)
  target_link_libraries(shurufa-neural-bench PRIVATE shurufa_neural)
  set_target_properties(shurufa-neural-bench PROPERTIES
    BUILD_RPATH "@executable_path;${SHURUFA_LLAMA_LIB_DIR}")

  # 独立无特权后台神经解码子进程：打包至 `Shurufa.app/Contents/Helpers/ShurufaNeuralService`，
  # 共享库 `libshurufa_neural.dylib` 打包至 `Contents/Frameworks/`，与主输入法进程彻底隔离 Metal 显存。
  add_executable(ShurufaNeuralService src/neural/ShurufaNeuralServiceMain.mm)
  target_compile_features(ShurufaNeuralService PRIVATE cxx_std_17)
  target_compile_options(ShurufaNeuralService PRIVATE -fobjc-arc -Wall -Wextra -Werror)
  target_link_libraries(ShurufaNeuralService PRIVATE shurufa_neural "-framework Foundation")
  set_target_properties(ShurufaNeuralService PROPERTIES
    BUILD_RPATH "@executable_path/../Frameworks;@executable_path;${SHURUFA_LLAMA_LIB_DIR}")

  # 不依赖 MSIMEClientInputMethod 的 POST_BUILD：仅改动 C++ 神经引擎时主可执行文件不重链，
  # POST_BUILD 不会触发，会导致包内 dylib/helper 静默滞后；改由独立目标按依赖同步拷贝到包内。
  add_custom_target(shurufa_stage_neural_bundle
    COMMAND "${CMAKE_COMMAND}" -E make_directory "$<TARGET_BUNDLE_DIR:MSIMEClientInputMethod>/Contents/Frameworks"
    COMMAND "${CMAKE_COMMAND}" -E make_directory "$<TARGET_BUNDLE_DIR:MSIMEClientInputMethod>/Contents/Helpers"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different "$<TARGET_FILE:shurufa_neural>" "$<TARGET_BUNDLE_DIR:MSIMEClientInputMethod>/Contents/Frameworks/libshurufa_neural.dylib"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different "$<TARGET_FILE:ShurufaNeuralService>" "$<TARGET_BUNDLE_DIR:MSIMEClientInputMethod>/Contents/Helpers/ShurufaNeuralService"
    DEPENDS shurufa_neural ShurufaNeuralService
    COMMENT "同步端侧神经解码库与独立后台服务到 Shurufa.app 包内")
  add_dependencies(MSIMEClientInputMethod shurufa_stage_neural_bundle)

  # UDS IPC 客户端（纯 Rust `msime_client`）与独立后台服务集成单测（使用 `--test-mock`，无 378 MB 模型依赖亦可完整验证崩溃自愈与帧协议）。
  add_executable(neural-service-ipc-test tests/neural/NeuralServiceIPCTest.mm)
  target_compile_features(neural-service-ipc-test PRIVATE cxx_std_17)
  target_compile_options(neural-service-ipc-test PRIVATE -fobjc-arc -Wall -Wextra -Werror -UNDEBUG)
  target_link_libraries(neural-service-ipc-test PRIVATE apple-client "-framework AppKit" "-framework Foundation")
  add_dependencies(neural-service-ipc-test ShurufaNeuralService)
  add_test(NAME neural-service-ipc COMMAND neural-service-ipc-test "$<TARGET_FILE:ShurufaNeuralService>")
  set_tests_properties(neural-service-ipc PROPERTIES TIMEOUT 30)
endif()

# 构建产物不能成为系统按 bundle id 查找的输入法副本。
if(MSIME_EDITION STREQUAL "shurufa")
  add_custom_command(TARGET MSIMEClientInputMethod POST_BUILD
    COMMAND "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/scripts/shurufa_installation.py" unregister "$<TARGET_BUNDLE_DIR:MSIMEClientInputMethod>")
endif()
