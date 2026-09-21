"""
Replace all app icons with the new logo from icon/4096.png.

Generates all required sizes for Tauri (Windows, macOS, Linux) including
tray icons, and updates the frontend logo.
"""
import os
from PIL import Image

SRC = r"D:\bian\donutbrowser-latest\icon\4096.png"
ICONS_DIR = r"D:\bian\donutbrowser-latest\src-tauri\icons"
PUBLIC_DIR = r"D:\bian\donutbrowser-latest\public"
ASSETS_DIR = r"D:\bian\donutbrowser-latest\assets"
TRAY_SRC_DIR = r"D:\bian\donutbrowser-latest\icon"


def resize_png(src_img, size, out_path):
    """Resize image to a square PNG with high quality."""
    img = src_img.copy()
    # Ensure RGBA for transparency support
    if img.mode != "RGBA":
        img = img.convert("RGBA")
    img = img.resize((size, size), Image.LANCZOS)
    img.save(out_path, "PNG")
    print(f"  ✓ {os.path.basename(out_path)} ({size}x{size})")


def generate_ico(src_img, out_path, sizes=(16, 32, 48, 64, 128, 256)):
    """Generate a multi-size .ico file for Windows."""
    if src_img.mode != "RGBA":
        src_img = src_img.convert("RGBA")
    icons = []
    for size in sizes:
        icon = src_img.resize((size, size), Image.LANCZOS)
        icons.append(icon)
    icons[0].save(out_path, format="ICO", sizes=[(s, s) for s in sizes])
    print(f"  ✓ {os.path.basename(out_path)} (multi-size)")


def main():
    print("Loading source image:", SRC)
    src = Image.open(SRC)
    print(f"  Source size: {src.size}, mode: {src.mode}")
    print()

    # --- Tauri app icons ---
    print("Generating Tauri app icons...")
    app_icon_sizes = [
        (32, "32x32.png"),
        (64, "64x64.png"),
        (128, "128x128.png"),
        (256, "128x128@2x.png"),
        (512, "icon.png"),
    ]
    for size, filename in app_icon_sizes:
        resize_png(src, size, os.path.join(ICONS_DIR, filename))

    # Windows .ico
    generate_ico(src, os.path.join(ICONS_DIR, "icon.ico"))

    # macOS .icns — Pillow can't write .icns natively, skip for now
    # (icon.icns is only needed for macOS builds, user can regenerate on Mac)
    print("  ⚠ icon.icns skipped (Pillow can't write .icns, regenerate on macOS)")

    # --- Windows Store / tile icons ---
    print("\nGenerating Windows tile icons...")
    tile_sizes = [
        30, 44, 71, 89, 107, 142, 150, 284, 310,
    ]
    for size in tile_sizes:
        filename = f"Square{size}x{size}Logo.png"
        resize_png(src, size, os.path.join(ICONS_DIR, filename))

    # StoreLogo.png (50x50 is common, use 128 for safety)
    resize_png(src, 128, os.path.join(ICONS_DIR, "StoreLogo.png"))

    # --- Tray icons ---
    print("\nCopying tray icons from icon/ directory...")
    tray_files = [
        "tray-icon-22.png",
        "tray-icon-44.png",
        "tray-icon-win-44.png",
        "tray-icon.svg",
    ]
    for f in tray_files:
        src_path = os.path.join(TRAY_SRC_DIR, f)
        dst_path = os.path.join(ICONS_DIR, f)
        if os.path.exists(src_path):
            import shutil
            shutil.copy2(src_path, dst_path)
            print(f"  ✓ {f}")
        else:
            print(f"  ⚠ {f} not found in {TRAY_SRC_DIR}")

    # Generate tray-icon-linux-44.png (from 44px tray icon or fallback to main)
    tray_44 = os.path.join(TRAY_SRC_DIR, "tray-icon-44.png")
    if os.path.exists(tray_44):
        import shutil
        shutil.copy2(tray_44, os.path.join(ICONS_DIR, "tray-icon-linux-44.png"))
        print(f"  ✓ tray-icon-linux-44.png (copied from tray-icon-44.png)")

    # --- Frontend logo ---
    print("\nUpdating frontend logos...")
    resize_png(src, 256, os.path.join(PUBLIC_DIR, "logo.png"))
    resize_png(src, 512, os.path.join(ASSETS_DIR, "logo.png"))

    # src-tauri/icons/logo.png
    resize_png(src, 512, os.path.join(ICONS_DIR, "logo.png"))

    print("\n✅ Done! All icons updated.")
    print("\nNote: icon.icns (macOS) needs to be regenerated on a Mac.")
    print("      Run: iconutil -c icns icon.iconset (after creating iconset folder)")


if __name__ == "__main__":
    main()
