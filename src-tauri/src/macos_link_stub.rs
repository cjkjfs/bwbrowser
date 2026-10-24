// macOS 链接桩：ort 预编译包（13.4 SDK 构建）的 Objective-C++ 目标文件引用
// ___isPlatformVersionAtLeast——该符号来自 clang 的 compiler-rt，而 rustc 链接
// 用自己的 compiler_builtins 不提供它，导致 Undefined symbols 链接失败。
// 我们的部署目标已是 macOS 14（.cargo/config.toml 钉死），ort 内部所有
// 平台版本检查必然为真，因此桩恒返 true 是正确语义。
#[cfg(all(target_os = "macos", feature = "coreml"))]
#[no_mangle]
pub extern "C" fn ___isPlatformVersionAtLeast(_major: u32, _minor: u32, _patch: u32) -> bool {
  true
}
