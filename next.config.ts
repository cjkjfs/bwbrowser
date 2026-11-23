import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  reactStrictMode: true,
  output: "export",
  images: {
    unoptimized: true,
  },
  // 2026-11-23: dev 产物移出 dist/（.next-dev）—— tauri.conf.json 的
  // frontendDist 指向 ../dist，generate_context!() 宏编译期会逐个读取
  // dist 下的文件；next dev 的 turbopack 缓存（dist/dev/cache）文件易变，
  // 被 turbopack 清理后宏读取即报 "failed to read asset ... os error 2"，
  // 导致 clippy/pre-commit 失败。生产构建（NODE_ENV=production）仍写 dist/。
  distDir: process.env.NODE_ENV === "production" ? "dist" : ".next-dev",
  compiler: {
    removeConsole: process.env.NODE_ENV === "production",
  },
};

export default nextConfig;
