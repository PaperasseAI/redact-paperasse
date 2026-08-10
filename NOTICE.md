# Third-party licenses

`paperasse-privacy` is MIT-licensed (see `LICENSE`). This is a summary of the
licenses used by its dependency tree, generated with `cargo license
--all-features` against the full workspace (native + wasm32 targets, every
feature including `tier-b`) and checked for anything that would be
incompatible with MIT distribution.

**Result: every dependency is permissively licensed. No GPL, AGPL, or SSPL
anywhere in the tree.** Two entries are worth calling out explicitly:

- **`r-efi`** is offered under `Apache-2.0 OR LGPL-2.1-or-later OR MIT` — a
  choice of licenses, not a requirement to use LGPL. This project (and every
  other consumer via Cargo's default resolution) uses it under Apache-2.0/MIT.
- **`resvg`/`usvg`** (pulled in transitively via `liteparse`'s image
  rendering) are `MPL-2.0`, a weak (file-level) copyleft: it only requires
  that *modifications to MPL-covered files* be shared if distributed. Neither
  crate is forked or modified here, so this doesn't extend any obligation to
  `paperasse-privacy`'s own code.

Regenerate this report with `cargo license --all-features` from the repo
root; CI's `license-check` job (`.github/workflows/ci.yml`) re-runs it on
every push and fails if a `GPL`/`AGPL`/`SSPL` token ever appears, so this
file can go stale on exact crate lists but not on the "nothing copyleft-heavy
snuck in" guarantee.

## By license

- **Apache-2.0**: allsorts, liteparse, liteparse-pdfium, liteparse-pdfium-sys, openssl, pyo3-async-runtimes, sync_wrapper, unicode-canonical-combining-class, unicode-general-category, unicode-joining-type, zopfli
- **Apache-2.0 OR MIT** (the overwhelming majority — 294 crates): the standard Rust ecosystem dual-license, including anydoc's and liteparse's own core dependencies (serde, tokio, regex, image, reqwest, clap, pyo3, wasm-bindgen, etc.)
- **Apache-2.0 OR MIT OR Zlib**: bytemuck, lru-slab, miniz_oxide, tinyvec, tinyvec_macros, zune-core, zune-inflate, zune-jpeg
- **MIT** (99 crates): anydoc, calamine, fontdb, lopdf, napi, printpdf, quick-xml, tesseract-rs, tiff, tokio-util, zip, and this workspace's own 6 crates, among others
- **MIT OR Unlicense**: aho-corasick, byteorder, csv, jiff, memchr, walkdir
- **BSD-2-Clause**: arrayref, av1-grain, rav1e, v_frame
- **BSD-3-Clause**: alloc-no-stdlib, avif-serialize, exr, ravif, tiny-skia, and others
- **ISC**: libloading, rustls-webpki, untrusted
- **Zlib**: slotmap, zlib-rs
- **Unicode-3.0**: the icu4x family (icu_collections, icu_normalizer, icu_properties, etc.) — pulled in via idna/url
- **CDLA-Permissive-2.0**: webpki-root-certs, webpki-roots
- **MPL-2.0**: resvg, usvg (see note above)
- A handful of dual/triple-licensed edge cases (0BSD/Apache-2.0/MIT, BSL-1.0, CC0-1.0, LLVM-exception) — all permissive, none require action.

Full detail (every crate + its exact license expression): run `cargo license
--all-features` yourself, or see the `license-check` CI job's log output.
