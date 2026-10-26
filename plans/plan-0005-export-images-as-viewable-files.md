# Plan: Export an Image as a Viewable File

_Filed 25-Oct-2026 from Obsidian-Brain’s plan-0019 (Chris’s ruling on ask `ai-front-door-fetch-gate-and/0020`: the KCS upload check OCRs every image in a PDF)._

## Problem

`--extract-stream N --output f` writes an image XObject’s stream, decoded through its non-image filters.
- A **JPEG** (`DCTDecode`) comes out as a JPEG file, which an OCR engine reads directly.
- A **Flate-compressed** image comes out as raw pixel rows, with no header (verified 25-Oct-2026 on v0.29.0: a 40×20 `DeviceGray` image extracted as exactly 800 bytes).

A caller that wants to look at the image, or OCR it, must therefore know the width, height, bits per component and colour space from `--images --json`, and build an image file itself.  That is easy for 8-bit `DeviceGray` and `DeviceRGB`, where the header is one line, and is real PDF knowledge for everything else:
- `Indexed` colour spaces, whose palette lives in another object;
- `DeviceCMYK`, and `ICCBased` with its `/N` and alternate;
- 1-bit images, with `/Decode` arrays and `/ImageMask`;
- 16-bit samples;
- `/SMask` and `/Mask`;
- image filters `pdf-dump` does not decode: `CCITTFaxDecode` and `JBIG2Decode` (common in office-scanner PDFs) and `JPXDecode`.

So today the KCS check (Obsidian-Brain `webapp/deploy/kcs-mail/mailgate.py`) can OCR only JPEGs and 8-bit grey or RGB images.  It holds any other file for a reviewer, as unreadable.  That is correct, since a file it could not read must not pass as clean, but it holds every PDF that carries, say, a palette logo or a fax-coded scan.

Two more gaps surfaced in the review of that check (25-Oct-2026):
- **Predictors are not undone.**  `--extract-stream` leaves a PNG predictor’s row-tag bytes in place, so 8×8 samples come out as 72 bytes, not 64.  A TIFF predictor (`/Predictor 2`) leaves horizontal differences of exactly the right length.  So a caller cannot tell a predicted image from a plain one by its length.  The check now refuses any `/Predictor` other than 1, and any `/Decode` array, reading them from `--object N --json`.
- **Inline images (`BI … ID … EI`) are not listed** by `--images`, and have no object number to extract.  The check finds them with `--search 'regex=(?s)(^|\s)BI\s.*?\sID\s'`, which also reaches form XObjects, and holds the file.

## Proposed Change

A mode that writes one image XObject as a file an ordinary viewer or OCR engine opens:

```
pdf-dump f.pdf --extract-image 12 --output page1-img.png
```

- **The output is PNG for decoded pixels**, and the stream as it is for `DCTDecode` (`.jpg`) and `JPXDecode` (`.jp2`).  The output’s extension must match what is written; a mismatch is a usage error, so a caller never OCRs a file named `.png` that holds JPEG bytes.
- **Colour:**
  - `Indexed` is expanded through its palette;
  - `DeviceCMYK` and `ICCBased` are converted to RGB, naively, which is enough for viewing and OCR;
  - 1-bit images honour `/Decode` and `/ImageMask`;
  - 16-bit samples are reduced to 8.
- **Fax and JBIG2:** `CCITTFaxDecode` is decoded (a crate exists, or it is a modest decoder); `JBIG2Decode` is decoded if a maintained crate allows, else refused.
- **A refusal is loud and specific:** a filter or colour space it cannot convert exits **1**, naming what it is.  It never writes a partial or wrong image, and never exits 0 with nothing written.
- **`--json`** reports the written path, the format, the pixel size and the source colour space and filter.
- **Predictors and `/Decode`** are applied, so the written image is the picture as drawn.
- **Inline images** are listed by `--images`, with their page and position in the content stream, and can be exported the same way, addressed by page and index rather than object number.

## Implementation Notes

- The colour-space resolution already exists for `--images` (it reports `color_space` and `bits_per_component`).  The new code is the sample unpacking and the PNG encoder; the `png` crate, which is lean, does the encoding (vet it with `crate-architect` per `/add-crate`).
- Pin each colour space and filter with a fixture PDF, and assert the decoded pixels, not only the exit code.
- **Exit codes,** per the portfolio table: 0 written; 1 not an image, not present, or not convertible (each named); 2 a malformed argument.

## Why Not a Workaround

Decoding PDF image formats is PDF knowledge, which is pdf-dump’s domain: palettes, CMYK, decode arrays, fax and JBIG2.  A script in the caller would duplicate it beside the tool, and drift from it.  The KCS check wraps only the two formats that need no PDF knowledge, 8-bit grey and RGB, in a one-line image header; everything else waits for this plan, and is held for a reviewer meanwhile.
