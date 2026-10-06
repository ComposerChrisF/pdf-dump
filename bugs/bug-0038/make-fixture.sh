#!/bin/sh
# Build ligature-gap.pdf: a one-page PDF whose only font has a /ToUnicode CMap
# that maps every code the page shows EXCEPT 0x2A — the shape of a Pages/Quartz
# export, where the "ti" and "tt" ligature glyphs get codes with no ToUnicode
# entry.  The page text is "Administra*ve Commi*ee Report" (0x2A standing in
# for the ligature glyphs), so 2 of 28 codes are unmapped: 7%, under
# pdf-dump's 20% low-coverage threshold.  Hand-written, uncompressed, with a
# correct xref, so nothing in it needs repairing.
set -eu
out="${1:-ligature-gap.pdf}"
cmap='/CIDInit /ProcSet findresource begin
12 dict begin
begincmap
/CMapName /Adobe-Identity-UCS def
/CMapType 2 def
1 begincodespacerange
<00> <FF>
endcodespacerange
2 beginbfrange
<20> <29> <0020>
<2B> <7E> <002B>
endbfrange
endcmap
CMapName currentdict /CMap defineresource pop
end
end'
content='BT /F1 24 Tf 72 700 Td (Administra*ve Commi*ee Report) Tj ET'
{
  printf '%%PDF-1.4\n'
  printf '1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n'
  printf '2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n'
  printf '3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 6 0 R >>\nendobj\n'
  printf '4 0 obj\n<< /Type /Font /Subtype /TrueType /BaseFont /Helvetica /FirstChar 32 /LastChar 126 /ToUnicode 5 0 R >>\nendobj\n'
  printf '5 0 obj\n<< /Length %d >>\nstream\n%s\nendstream\nendobj\n' "$(printf '%s' "$cmap" | wc -c | tr -d ' ')" "$cmap"
  printf '6 0 obj\n<< /Length %d >>\nstream\n%s\nendstream\nendobj\n' "$(printf '%s' "$content" | wc -c | tr -d ' ')" "$content"
} > "$out.body"
xref=$(wc -c < "$out.body" | tr -d ' ')
{
  cat "$out.body"
  printf 'xref\n0 7\n0000000000 65535 f \n'
  for n in 1 2 3 4 5 6; do
    off=$(grep -abo "^$n 0 obj" "$out.body" | cut -d: -f1)
    printf '%010d 00000 n \n' "$off"
  done
  printf 'trailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n' "$xref"
} > "$out"
rm "$out.body"
