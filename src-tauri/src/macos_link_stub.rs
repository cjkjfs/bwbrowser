// macOS 链接桩：ort 预编译包（13.4 SDK 构建）的 Objective-C++ 目标文件引用
// clang compiler-rt 的平台版本检查函数——rustc 链接用自己的 compiler_builtins
// 不提供它，导致 Undefined symbols 链接失败。
//
// 符号名注意 Mach-O 的 leading-underscore 惯例：C 源码里叫
// __isPlatformVersionAtLeast（双下划线），编译后 Mach-O 符号是
// ___isPlatformVersionAtLeast（三下划线，即链接器报错里的名字）。
// 所以 Rust 侧签名用双下划线，rustc 会自动补那一个前导下划线。
//
// 语义：我们的部署目标已是 macOS 14（.cargo/config.toml 钉死），ort 内部
// 所有平台版本检查必然为真，恒返 true 是正确行为。
#[cfg(all(target_os = "macos", feature = "coreml"))]
#[no_mangle]
pub extern "C" fn __isPlatformVersionAtLeast(_major: u32, _minor: u32, _patch: u32) -> bool {
  true
}
