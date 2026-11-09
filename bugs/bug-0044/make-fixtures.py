import sys
def pdf(content: bytes, path):
    widths = " ".join(["600"]*95)
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        b"<< /Length %d >>\nstream\n" % len(content) + content + b"\nendstream",
        ("<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding /FirstChar 32 /LastChar 126 /Widths [%s] >>" % widths).encode(),
    ]
    out = bytearray(b"%PDF-1.7\n"); offs = []
    for i, o in enumerate(objs, 1):
        offs.append(len(out)); out += b"%d 0 obj\n" % i + o + b"\nendobj\n"
    x = len(out)
    out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs)+1)
    for o in offs: out += b"%010d 00000 n \n" % o
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs)+1, x)
    open(path, "wb").write(out)

lines = [("Statement Period", 700), ("Account 1234 5678", 680)]
def glyphs():
    for text, y in lines:
        x = 72.0
        for ch in text:
            if ch == " ": x += 7.2*1.5; continue   # a word gap, no space glyph
            yield ch, x, y; x += 7.2               # 12pt Courier advance
d = sys.argv[1]
pdf(b"\n".join(b"BT /F1 12 Tf 1 0 0 1 %.2f %d Tm (%s) Tj ET" % (x, y, c.encode()) for c, x, y in glyphs()), d + "/bt-per-glyph.pdf")
pdf(b"BT /F1 12 Tf\n" + b"\n".join(b"1 0 0 1 %.2f %d Tm (%s) Tj" % (x, y, c.encode()) for c, x, y in glyphs()) + b"\nET", d + "/tm-per-glyph.pdf")
body, px, py = [b"BT /F1 12 Tf"], 0.0, 0
for c, x, y in glyphs():
    body.append(b"%.2f %d Td (%s) Tj" % (x-px, y-py, c.encode())); px, py = x, y
pdf(b"\n".join(body + [b"ET"]), d + "/td-per-glyph.pdf")
