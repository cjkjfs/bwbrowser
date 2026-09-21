import type { CSSProperties } from "react";

export const Logo = ({
  width,
  height,
  className,
  style,
}: {
  width?: number | string;
  height?: number | string;
  className?: string;
  style?: CSSProperties;
}) => (
  // biome-ignore lint/performance/noImgElement: static asset
  <img
    src="/logo.png"
    alt="Logo"
    width={width}
    height={height}
    className={className}
    style={style}
  />
);
