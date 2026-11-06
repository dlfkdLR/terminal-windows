#!/usr/bin/env python3
"""Generates crates/tw-cli/assets/AppIcon.icns: a four-pane logo on a rounded dark tile.
Pure Python plus macOS `iconutil`; run it only when the icon design changes."""
import os, struct, subprocess, sys, tempfile, zlib

def render(n, aa=2):
    """RGBA bytes of an n x n icon (aa x aa supersampling)."""
    def inside_round(x, y, size, r):
        cx = min(max(x, r), size - r); cy = min(max(y, r), size - r)
        return (x - cx) ** 2 + (y - cy) ** 2 <= r * r
    def lerp(a, b, t): return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3))
    tile_r = 0.225 * n
    pane = 0.215 * n; gap = 0.035 * n; start = 0.5 * n - pane - gap / 2
    panes = [(start, start), (start + pane + gap, start), (start, start + pane + gap), (start + pane + gap, start + pane + gap)]
    out = bytearray()
    for py in range(n):
        for px in range(n):
            r = g = b = a = 0
            for sy in range(aa):
                for sx in range(aa):
                    x = px + (sx + 0.5) / aa; y = py + (sy + 0.5) / aa
                    if not inside_round(x, y, n, tile_r):
                        continue
                    col = lerp((34, 40, 58), (14, 18, 30), y / n)        # tile background
                    for (ox, oy) in panes:
                        if ox <= x < ox + pane and oy <= y < oy + pane:
                            t = ((x - ox) + (y - oy)) / (2 * pane)
                            col = lerp((92, 200, 255), (0, 120, 212), t)  # blue pane
                            break
                    r += col[0]; g += col[1]; b += col[2]; a += 255
            k = aa * aa
            cov = a / 255
            out += bytes([int(r / cov) if cov else 0, int(g / cov) if cov else 0, int(b / cov) if cov else 0, int(a / k)])
    return bytes(out)

def downscale(rgba, n, m):
    f = n // m
    out = bytearray()
    for y in range(m):
        for x in range(m):
            acc = [0, 0, 0, 0]
            for dy in range(f):
                for dx in range(f):
                    i = ((y * f + dy) * n + x * f + dx) * 4
                    a = rgba[i + 3]
                    acc[0] += rgba[i] * a; acc[1] += rgba[i + 1] * a; acc[2] += rgba[i + 2] * a; acc[3] += a
            a = acc[3]
            out += bytes([acc[0] // a if a else 0, acc[1] // a if a else 0, acc[2] // a if a else 0, a // (f * f)])
    return bytes(out)

def png(rgba, n):
    raw = b"".join(b"\x00" + rgba[y * n * 4:(y + 1) * n * 4] for y in range(n))
    def chunk(t, d): return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", n, n, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")

def main(out_path):
    big = render(1024)
    with tempfile.TemporaryDirectory() as tmp:
        iconset = os.path.join(tmp, "AppIcon.iconset"); os.mkdir(iconset)
        for base in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                px = base * scale
                data = big if px == 1024 else downscale(big, 1024, px)
                name = f"icon_{base}x{base}" + ("@2x" if scale == 2 else "") + ".png"
                open(os.path.join(iconset, name), "wb").write(png(data, px))
        subprocess.run(["iconutil", "-c", "icns", iconset, "-o", out_path], check=True)

if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "crates/tw-cli/assets/AppIcon.icns")
