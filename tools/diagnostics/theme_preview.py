from PIL import Image, ImageDraw, ImageFont

F_CN = "C:/Windows/Fonts/msyh.ttc"
F_MONO = "C:/Windows/Fonts/consola.ttf"


def cn(sz, bold=False):
    return ImageFont.truetype(F_CN, sz, index=1 if bold else 0)


def mono(sz):
    return ImageFont.truetype(F_MONO, sz)


themes = [
    dict(
        name="ChatGPT Dark",
        file="config/chatgpt.lua",
        active_bg="#ECECEC",
        bg="#212121",
        bar="#171717",
        fg="#ECECEC",
        dim="#7A7A7A",
        accent="#ECECEC",
        accent2="#C5C5C5",
        active_fg="#171717",
        inactive="#2A2A2A",
        inactive_fg="#9B9B9B",
        battery="#ECECEC",
        ansi=["#1A1A1A", "#B99F9F", "#9CAAA0", "#B5AD9B", "#A0A9B7", "#B0A5B5", "#99A8A8", "#C9C9C9"],
        bright=["#4A4A4A", "#D0B9B9", "#B7C2BA", "#CAC4B5", "#BCC3CE", "#C9C1CD", "#B2BFBF", "#FFFFFF"],
    ),
    dict(
        name="Claude Dark",
        file="config/claude.lua",
        active_bg="#D97757",
        bg="#1F1E1D",
        bar="#1A1918",
        fg="#F0EEE6",
        dim="#9C958C",
        accent="#D97757",
        accent2="#E89478",
        active_fg="#1F1E1D",
        inactive="#2B2A28",
        inactive_fg="#A39C93",
        battery="#86B36B",
        ansi=["#1A1918", "#DE6B57", "#86B36B", "#E0B36A", "#7C9FD4", "#C08BB5", "#74B3A6", "#D8D3C8"],
        bright=["#4A4640", "#E89478", "#A6D48C", "#F2CD8C", "#9BBCEA", "#DDA9CF", "#94D2C4", "#FFFDF7"],
    ),
    dict(
        name="Gemini Dark",
        file="config/gemini.lua",
        active_bg="#1A73E8",
        bg="#131314",
        bar="#0F0F10",
        fg="#E8EAED",
        dim="#9AA0A6",
        accent="#8AB4F8",
        accent2="#D96570",
        active_fg="#FFFFFF",
        inactive="#1E1F20",
        inactive_fg="#9AA0A6",
        battery="#81C995",
        ansi=["#131314", "#F28B82", "#81C995", "#FDD663", "#8AB4F8", "#C58AF9", "#7DD3E8", "#DADCE0"],
        bright=["#3C4043", "#F6AEA9", "#A8DAB5", "#FDE293", "#AECBFA", "#D7AEFB", "#9BE7F3", "#FFFFFF"],
    ),
]

W = 1400
PH = 240
PAD = 26
out = Image.new("RGB", (W, PH * 3 + 34), (12, 12, 13))
d = ImageDraw.Draw(out)

for i, t in enumerate(themes):
    y = 16 + i * PH
    panel = (PAD, y, W - PAD, y + PH - 16)
    d.rounded_rectangle(panel, radius=10, fill=t["bg"], outline="#2C2C2C")

    d.text((PAD + 20, y + 12), t["name"], font=cn(20, True), fill=t["fg"])
    d.text((PAD + 20, y + 40), t["file"], font=mono(13), fill=t["dim"])

    bx0, bx1 = PAD + 20, W - PAD - 20
    by0, by1 = y + 66, y + 110
    d.rounded_rectangle((bx0, by0, bx1, by1), radius=6, fill=t["bar"])

    ax0, ax1 = bx0 + 8, bx0 + 196
    d.rounded_rectangle((ax0, by0 + 6, ax1, by1 - 6), radius=6, fill=t["active_bg"])
    d.text((ax0 + 14, by0 + 15), "PowerShell", font=mono(15), fill=t["active_fg"])

    ix0, ix1 = ax1 + 8, ax1 + 132
    d.rounded_rectangle((ix0, by0 + 6, ix1, by1 - 6), radius=6, fill=t["inactive"])
    d.text((ix0 + 14, by0 + 15), "cmd", font=mono(15), fill=t["inactive_fg"])

    segs = [
        ("\u25cf \u665a\u4e0a\u597d", t["accent"]),
        ("   \u00b7   ", t["dim"]),
        ("20:31", t["fg"]),
        ("  11/09 \u5468\u4e00  ", t["dim"]),
        ("\u00b7", t["dim"]),
        ("  \u25cf 100%", t["battery"]),
        ("  \u00b7  ", t["dim"]),
        ("Elaina-SakiOVO", t["accent2"]),
        ("  ", t["dim"]),
    ]
    f = cn(15)
    widths = [d.textlength(s, font=f) for s, _ in segs]
    x = bx1 - 14 - sum(widths)
    for (s, c), w in zip(segs, widths):
        d.text((x, by0 + 17), s, font=f, fill=c)
        x += w

    sw_y = by1 + 14
    sw_h = 30
    gap = 6
    sw_w = (bx1 - bx0 - gap * 15) / 16
    for k in range(16):
        col = t["ansi"][k] if k < 8 else t["bright"][k - 8]
        x0 = bx0 + k * (sw_w + gap)
        d.rounded_rectangle((x0, sw_y, x0 + sw_w, sw_y + sw_h), radius=4, fill=col)
        d.text((x0 + 3, sw_y + sw_h + 3), col.replace("#", ""), font=mono(9), fill=t["dim"])

    ty = sw_y + sw_h + 24
    d.text((bx0, ty), "PS C:\\Users>", font=mono(15), fill=t["ansi"][4])
    d.text((bx0 + 128, ty), "git status", font=mono(15), fill=t["fg"])
    d.text((bx0 + 236, ty), "--short", font=mono(15), fill=t["ansi"][3])
    d.text((bx0 + 318, ty), "# \u4fee\u6539: 3 \u4e2a\u6587\u4ef6", font=cn(15), fill=t["dim"])
    d.text((bx0 + 500, ty), "error:", font=mono(15), fill=t["ansi"][1])
    d.text((bx0 + 572, ty), "permission denied", font=mono(15), fill=t["ansi"][1])

out.save("target/theme-preview.png")
print(out.size)
