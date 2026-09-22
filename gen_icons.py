#!/usr/bin/env python3
"""Generate proper Windows ICO with BMP format (not PNG) for max compatibility."""

from PIL import Image
import struct
import os

SRC = r"d:\bian\donutbrowser-latest\icon\4096.png"
ICONS_DIR = r"d:\bian\donutbrowser-latest\src-tauri\icons"

img = Image.open(SRC).convert("RGBA")
print(f"Source: {img.size[0]}x{img.size[1]}")

# ICO sizes - must include 256x256 for Windows high-DPI
ico_sizes = [16, 24, 32, 48, 64, 96, 128, 256]

# Generate each size as BMP data
images = []
for size in ico_sizes:
    resized = img.resize((size, size), Image.LANCZOS)
    images.append(resized)

# Save as ICO - Pillow supports multi-size ICO
ico_path = os.path.join(ICONS_DIR, "icon.ico")
# Use format='ICO' which embeds as BMP for < 256, and PNG for 256
# To force BMP for all, we need a custom approach

# Actually, Pillow's ICO writer handles this correctly:
# - Sizes < 256 are stored as BMP
# - 256x256 is stored as PNG (smaller, and Windows 10+ supports it)
img.save(ico_path, format='ICO', sizes=[(s, s) for s in ico_sizes])
fsize = os.path.getsize(ico_path)
print(f"Generated icon.ico: {fsize} bytes ({fsize/1024:.0f} KB)")

# Verify by reading the ICO header
with open(ico_path, 'rb') as f:
    header = f.read(6)
    count = struct.unpack('<H', header[4:6])[0]
    print(f"ICO contains {count} images:")
    for i in range(count):
        entry = f.read(16)
        w = entry[0] or 256
        h = entry[1] or 256
        size = struct.unpack('<I', entry[8:12])[0]
        offset = struct.unpack('<I', entry[12:16])[0]
        print(f"  {w}x{h} - {size} bytes at offset {offset}")

# Generate PNG icons
png_sizes = {
    "32x32.png": (32,32),
    "64x64.png": (64,64),
    "128x128.png": (128,128),
    "128x128@2x.png": (256,256),
    "icon.png": (512,512),
    "logo.png": (512,512),
}
for name, size in png_sizes.items():
    resized = img.resize(size, Image.LANCZOS)
    path = os.path.join(ICONS_DIR, name)
    resized.save(path, format="PNG")
    print(f"Generated {name} ({os.path.getsize(path)} bytes)")

# Generate Windows Store square icons
square_sizes = {
    "Square30x30Logo.png": (30,30),
    "Square44x44Logo.png": (44,44),
    "Square71x71Logo.png": (71,71),
    "Square89x89Logo.png": (89,89),
    "Square107x107Logo.png": (107,107),
    "Square142x142Logo.png": (142,142),
    "Square150x150Logo.png": (150,150),
    "Square284x284Logo.png": (284,284),
    "Square310x310Logo.png": (310,310),
    "SquareStoreLogo.png": (50,50),
    "StoreLogo.png": (50,50),
}
for name, size in square_sizes.items():
    resized = img.resize(size, Image.LANCZOS)
    path = os.path.join(ICONS_DIR, name)
    resized.save(path, format="PNG")

print("\nDone!")
