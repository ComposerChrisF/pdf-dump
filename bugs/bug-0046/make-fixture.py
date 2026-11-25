#!/usr/bin/env python3
"""Write simple-font-2byte-codespace.pdf: one page, a simple (non-CID) Helvetica
font with /WinAnsiEncoding whose /ToUnicode CMap declares a 2-byte codespace
(<0000> <FFFF>) while its bfchar entries are 1-byte (<48> <0048>).  InDesign
writes this shape; Preview and Acrobat extract the text correctly.

Run: python3 -I make-fixture.py  (writes next to this script)."""
import os, zlib

TEXT = b"Hello, World 2026"
chars = sorted(set(TEXT))
bfchar = b"".join(b"<%02X> <%04X>\n" % (c, c) for c in chars)
cmap = (b"/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n"
        b"/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n"
        b"/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n"
        b"1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n"
        + b"%d beginbfchar\n" % len(chars) + bfchar + b"endbfchar\n"
        b"endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n")
content = b"BT /F1 24 Tf 72 700 Td (" + TEXT + b") Tj ET\n"

def stream(data):
    z = zlib.compress(data)
    return b"<< /Length %d /Filter /FlateDecode >>\nstream\n" % len(z) + z + b"\nendstream"

objs = [
    b"<< /Type /Catalog /Pages 2 0 R >>",
    b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] "
    b"/Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>",
    stream(content),
    b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica "
    b"/Encoding /WinAnsiEncoding /ToUnicode 6 0 R >>",
    stream(cmap),
]
out = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
offsets = []
for i, o in enumerate(objs, 1):
    offsets.append(len(out))
    out += b"%d 0 obj\n" % i + o + b"\nendobj\n"
xref = len(out)
out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)
out += b"".join(b"%010d 00000 n \n" % off for off in offsets)
out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, xref)
path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "simple-font-2byte-codespace.pdf")
with open(path, "wb") as f:
    f.write(out)
print(path)
