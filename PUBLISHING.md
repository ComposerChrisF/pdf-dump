# Publishing pdf-dump to crates.io

pdf-dump is part of the **PDF crate family** but is **independent** — it has no
`medpdf` dependencies, so it can be published on its own at any time, in any
order.  The authoritative family procedure and the `publish-status` tool live in
the medpdf repo: `../medpdf/PUBLISHING.md`.

The model is **publish-only**: `verset` / the `/commit-*` skills own the version
number; cargo-release only uploads and tags the already-committed current
version.  Every command is dry-run by default — add `-x` to execute.

```
cargo release publish        # dry-run preview
cargo release publish -x      # upload the current version
cargo release tag -x          # tag pdf-dump-v<version>
cargo release push -x         # push the tag
```

Do **not** run `cargo release <level>` or `cargo release version` — that bumps
the version and collides with the commit-skill workflow.  Config lives in
`release.toml`.
