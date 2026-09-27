#!/usr/bin/env python3
"""Regenerate `assets/logo.svg` from the installed WeChat icon.

The bubble artwork is not hand-drawn: it is traced from the real icon in
Weixin.exe, so it matches the official mark (bubble shapes, the green gap
between the two bubbles, the eyes, the corner radius). The download badge in
the bottom-right corner is added by this script on top of the traced artwork.

Usage (Windows, from the repository root):

    python tools/trace_icon.py
    python tools/trace_icon.py --exe "C:\\Program Files\\Tencent\\Weixin\\Weixin.exe"
    python tools/trace_icon.py --png some-icon.png    # skip the extraction step

Requirements: Pillow (PNG output / inspection) and OpenCV (`pip install opencv-python`).
Both are development-only: the app itself does not depend on them.
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.wintypes as wintypes
import os
import sys

import cv2
import numpy as np
from PIL import Image

DEFAULT_EXE = r"C:\Program Files\Tencent\Weixin\Weixin.exe"

# Output grid: the SVG viewBox is 48x48.
UNITS = 48.0
# Extracted icon size, in pixels.
ICO_SIZE = 256
# Corner radius of the tile, measured from the source icon (57/256 of the width).
CORNER_RADIUS = 10.7
# Nudge for the traced bubbles, to leave the bottom-right corner free for the badge.
BUBBLE_SHIFT = (-2.1, -2.1)

BRAND = "#07C160"
WHITE = "#FFFFFF"

# The download badge, in 48x48 units.
BADGE = f"""    <!-- 右下角下载标识：绿圆 + 白箭头，白色描边与气泡/底色分隔 -->
    <circle cx="40.2" cy="40.2" r="6.2" fill="{BRAND}" stroke="{WHITE}" stroke-width="1.5"/>
    <g fill="{WHITE}">
      <rect x="39.5" y="37.3" width="1.5" height="3.7"/>
      <path d="M38.35 39.9 40.2 41.75l1.85-1.85Z"/>
      <rect x="38.35" y="42.4" width="3.7" height="1.5"/>
    </g>"""


def extract_icon(exe: str, size: int) -> Image.Image:
    """Pull the icon resource out of a Windows executable via the Win32 API."""
    if sys.platform != "win32":
        raise SystemExit("icon extraction needs Windows; pass --png instead")

    user32 = ctypes.windll.user32
    gdi32 = ctypes.windll.gdi32
    extract = user32.PrivateExtractIconsW
    extract.argtypes = [
        wintypes.LPCWSTR,
        ctypes.c_int,
        ctypes.c_int,
        ctypes.c_int,
        ctypes.POINTER(wintypes.HANDLE),
        ctypes.POINTER(wintypes.UINT),
        ctypes.c_uint,
        ctypes.c_uint,
    ]
    extract.restype = ctypes.c_uint

    hicon = wintypes.HANDLE()
    count = wintypes.UINT()
    if not extract(exe, 0, size, size, ctypes.byref(hicon), ctypes.byref(count), 1, 0):
        raise SystemExit(f"no icon resource found in {exe}")

    class BITMAPINFOHEADER(ctypes.Structure):
        _fields_ = [
            ("biSize", wintypes.DWORD),
            ("biWidth", wintypes.LONG),
            ("biHeight", wintypes.LONG),
            ("biPlanes", wintypes.WORD),
            ("biBitCount", wintypes.WORD),
            ("biCompression", wintypes.DWORD),
            ("biSizeImage", wintypes.DWORD),
            ("biXPelsPerMeter", wintypes.LONG),
            ("biYPelsPerMeter", wintypes.LONG),
            ("biClrUsed", wintypes.DWORD),
            ("biClrImportant", wintypes.DWORD),
        ]

    class BITMAPINFO(ctypes.Structure):
        _fields_ = [("bmiHeader", BITMAPINFOHEADER), ("bmiColors", wintypes.DWORD * 3)]

    hdc = user32.GetDC(0)
    memdc = gdi32.CreateCompatibleDC(hdc)
    bitmap = gdi32.CreateCompatibleBitmap(hdc, size, size)
    previous = gdi32.SelectObject(memdc, bitmap)
    user32.DrawIconEx(memdc, 0, 0, hicon, size, size, 0, 0, 3)  # DI_NORMAL

    info = BITMAPINFO()
    info.bmiHeader.biSize = ctypes.sizeof(BITMAPINFOHEADER)
    info.bmiHeader.biWidth = size
    info.bmiHeader.biHeight = -size  # top-down
    info.bmiHeader.biPlanes = 1
    info.bmiHeader.biBitCount = 32
    info.bmiHeader.biCompression = 0  # BI_RGB

    buffer = ctypes.create_string_buffer(size * size * 4)
    gdi32.GetDIBits(memdc, bitmap, 0, size, buffer, ctypes.byref(info), 0)
    image = Image.frombuffer("RGBA", (size, size), buffer, "raw", "BGRA", 0, 1).copy()

    gdi32.SelectObject(memdc, previous)
    gdi32.DeleteObject(bitmap)
    gdi32.DeleteDC(memdc)
    user32.ReleaseDC(0, hdc)
    user32.DestroyIcon(hicon)
    return image


def trace(mask: np.ndarray, tolerance: float) -> list[str]:
    """Trace a binary mask into closed cubic-Bézier subpaths."""
    mask = cv2.morphologyEx(mask, cv2.MORPH_CLOSE, np.ones((3, 3), np.uint8))
    contours, _ = cv2.findContours(mask, cv2.RETR_CCOMP, cv2.CHAIN_APPROX_NONE)

    scale = UNITS / float(ICO_SIZE)
    subpaths: list[str] = []
    for contour in contours:
        points = cv2.approxPolyDP(contour, tolerance, True).reshape(-1, 2).astype(float) * scale
        points = points.tolist()
        n = len(points)
        path = f"M{points[0][0]:.2f} {points[0][1]:.2f}"
        for i in range(n):
            p0 = points[(i - 1) % n]
            p1 = points[i]
            p2 = points[(i + 1) % n]
            p3 = points[(i + 2) % n]
            # Catmull-Rom style control points, converted to cubic Béziers.
            c1 = (p1[0] + (p2[0] - p0[0]) / 6.0, p1[1] + (p2[1] - p0[1]) / 6.0)
            c2 = (p2[0] - (p3[0] - p1[0]) / 6.0, p2[1] - (p3[1] - p1[1]) / 6.0)
            path += (
                f" C{c1[0]:.2f} {c1[1]:.2f} {c2[0]:.2f} {c2[1]:.2f} {p2[0]:.2f} {p2[1]:.2f}"
            )
        subpaths.append(path + "Z")
    return subpaths


def indent_path(path: str, indent: str) -> str:
    """Wrap a path's `C` segments onto their own lines for readability."""
    parts = path.split(" C")
    return ("\n" + indent).join([parts[0]] + ["C" + p for p in parts[1:]])


def build_svg(image: Image.Image) -> str:
    rgba = np.array(image.convert("RGBA"))
    r, g, b, a = rgba[:, :, 0], rgba[:, :, 1], rgba[:, :, 2], rgba[:, :, 3]

    # The mark is flat-coloured: white bubbles on a green tile.
    white = (((r > 170) & (g > 170) & (b > 170) & (a > 128)).astype(np.uint8)) * 255
    bubbles = trace(white, 0.9)

    body = "\n          ".join(indent_path(s, "          ") for s in bubbles)
    shift = f"translate({BUBBLE_SHIFT[0]} {BUBBLE_SHIFT[1]})"

    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48" width="48" height="48">
  <!--
    微信下载器图标：微信官方图标矢量化 + 右下角下载标识。
    气泡轮廓是从微信官方 exe 的图标资源里描摹出来的，因此保留了原版的双气泡
    形状、两气泡之间的绿色分隔缝与「眼睛」的位置。
    由 tools/trace_icon.py 生成，请勿手工编辑。
    绿色圆角底为 48x48、圆角 {CORNER_RADIUS}（等于原图 57/{ICO_SIZE}）。
    气泡整体略微平移让出右下角给下载标识。
    颜色由 build.rs 在编译期光栅化成 logo.png 使用（GPUI 的 SVG 渲染只取
    单色蒙版，会丢掉这些颜色）。
  -->
  <g>
    <!-- 微信绿圆角底 -->
    <rect width="48" height="48" rx="{CORNER_RADIUS}" fill="{BRAND}"/>

    <!-- 白色双气泡：从官方图标描摹；外轮廓与「眼睛」同路径，用 evenodd 挖空 -->
    <g transform="{shift}">
      <path fill-rule="evenodd" fill="{WHITE}"
            d="{body}"/>
    </g>

{BADGE}
  </g>
</svg>
"""


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", default=DEFAULT_EXE, help="path to Weixin.exe")
    parser.add_argument("--png", help="use an existing icon PNG instead of extracting")
    parser.add_argument("--out", default="assets/logo.svg", help="SVG to write")
    args = parser.parse_args()

    if args.png:
        image = Image.open(args.png)
    else:
        image = extract_icon(args.exe, ICO_SIZE)

    svg = build_svg(image)
    os.makedirs(os.path.dirname(args.out) or ".", exist_ok=True)
    with open(args.out, "w", encoding="utf-8") as handle:
        handle.write(svg)
    print(f"wrote {args.out} ({len(svg)} bytes)")


if __name__ == "__main__":
    main()
