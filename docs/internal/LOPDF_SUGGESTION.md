# lopdf: Encrypt Dictionary Consumed During Loading

## The Problem

When lopdf loads an encrypted PDF, it consumes the `/Encrypt` dictionary and removes it from the in-memory document model. This makes it impossible for API clients to inspect encryption metadata (algorithm, permissions, key length, etc.) after loading.

### What Happens On Disk

A modern PDF (1.5+) using cross-reference streams stores trailer entries in the XRef stream object's dictionary:

```
292 0 obj
<<
  /Type /XRef
  /Root 2 0 R
  /Info 290 0 R
  /Encrypt 291 0 R    % <-- reference to encrypt dict
  /Size 293
  ...
>>
stream
...
endstream
endobj
```

Object 291 is a normal encrypt dictionary:

```
291 0 obj
<<
  /Filter /Standard
  /V 4
  /R 4
  /Length 128
  /P -3904
  /O (...)
  /U (...)
>>
endobj
```

This is correct, spec-compliant PDF structure.

### What lopdf Does During Loading

1. Reads the XRef stream, finds `/Encrypt 291 0 R`
2. Reads object 291 to initialize its internal decryption state
3. **Removes** `/Encrypt` from `doc.trailer`
4. **Removes** object 291 from `doc.objects`
5. Leaves the XRef stream object (292) untouched, so its dict still contains `/Encrypt 291 0 R` — now a dangling reference

### The Impact

After loading, an API client that wants to answer "is this PDF encrypted, and with what settings?" has no straightforward way to do so:

- `doc.trailer.get(b"Encrypt")` returns nothing (entry removed)
- `doc.get_object((291, 0))` returns an error (object removed)
- The only surviving evidence is a dangling reference inside the XRef stream object's dictionary, which requires scanning all stream objects to find

This affects any tool that wants to report on PDF security properties — a common need for PDF inspection, validation, and debugging tools.

## Suggested Improvement

Preserve the encrypt dictionary in the document model after loading. Two possible approaches:

### Option A: Keep It in the Object Table (Minimal Change)

Simply stop removing the encrypt dictionary object from `doc.objects`. lopdf can still consume it internally for decryption, but the object would remain available for inspection. The `/Encrypt` entry in `doc.trailer` should also be preserved.

This is the smallest change and maintains backward compatibility — clients that don't care about encryption simply never look at it.

### Option B: Expose a Dedicated API (More Ergonomic)

Add a field or method to `Document` that exposes encryption metadata:

```rust
impl Document {
    /// Returns the encryption dictionary if the PDF is encrypted, or None.
    pub fn encrypt_dict(&self) -> Option<&Dictionary> { ... }

    /// Returns true if the document was encrypted.
    pub fn is_encrypted(&self) -> bool { ... }
}
```

This could be populated during loading (before the raw object is consumed) and would give clients clean access without needing to understand PDF trailer structure.

### Recommendation

Option A is the pragmatic fix — it's a small change (stop deleting two things) and immediately solves the problem for all clients. Option B is a nice addition on top but isn't strictly necessary if the raw data is accessible.

Either way, the current behavior of silently discarding encryption metadata creates a gap where a PDF that is clearly encrypted appears unencrypted to any tool built on lopdf.
