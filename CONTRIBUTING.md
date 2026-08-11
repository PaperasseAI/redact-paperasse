# Contributing to redact-paperasse

Thanks for wanting to help. This is a tool people point at payslips, invoices and
ID documents, so the bar here is a little different from a normal library: the
worst outcome is not a crash, it's **quietly returning a document with the
sensitive parts still on it**. Almost every convention below exists because of
that one asymmetry.

Everything here is a real convention the codebase already follows, not
aspiration. If you find code that contradicts this file, the code is the bug —
please say so.

---

## The one hard rule: never commit real personal data

**No real identifiers in this repository, ever.** Not in tests, not in fixtures,
not in a comment showing "what a real one looks like", not in a screenshot or a
demo GIF.

This is not hypothetical. An early version of this project used a real French
NIR as a test value — someone's actual social security number, taken from a real
letter. It reached six source files, was rendered into the PNG and PDF fixtures,
was burned into the animated GIF in the README, and was published to crates.io
and PyPI before anyone noticed. crates.io releases cannot be deleted, only
yanked, and yanked files stay downloadable. It is still out there.

So:

- Generate synthetic values that satisfy the checksum. Every validator in
  `crates/recognizers/` is documented well enough to construct a passing number
  from scratch.
- Payment-card tests use the standard gateway test numbers (`4111111111111111`
  and friends), which are Luhn-valid by construction and belong to nobody.
- If you are adding a fixture image or PDF, generate it — don't scan something.
- If you think you may have committed something real, **say so immediately**,
  before worrying about how it looks. Catching it pre-publish costs a
  force-push. Catching it post-publish costs nothing less than permanence.

---

## Getting set up

```bash
git clone https://github.com/PaperasseAI/redact-paperasse
cd redact-paperasse
cargo test --workspace --locked
```

The first build is slow — `liteparse` compiles Tesseract and Leptonica from
source through CMake, which is why `cmake` and `pkg-config` are prerequisites.
See the README's architecture section for why that dependency exists at all.

Before you open a PR, run exactly what CI runs:

```bash
cargo metadata --locked --format-version 1 > /dev/null   # lockfile is current
cargo fmt --all --check
cargo clippy --workspace --all-features --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Run the lockfile check **first**. Any unlocked cargo command silently rewrites
`Cargo.lock` on disk, so a `--locked` check placed after one is validating a file
the previous step just repaired — it can never fail. That exact ordering bug let
a stale lockfile reach a release tag and break publishing.

---

## Fixing a bug: prove it before you fix it

Write the failing test first, and confirm it fails **for the reason you think**.
This is not ceremony. Two of the more embarrassing episodes in this project's
history were:

- A fix that appeared not to work, where the code was right all along and the
  *test fixture* was wrong (PIL's `rotate()` is counter-clockwise, so a 90°
  rotation needs EXIF tag 6, not 8). Time went into debugging correct code.
- A confident diagnosis — "the OCR resolution is too low" — that turned out to be
  wrong the moment it was measured: 150 DPI and 300 DPI produced byte-identical
  output. The real cause was page segmentation.

A test that fails before your change and passes after is the only thing that
distinguishes a fix from a plausible story.

**Verify guards in both directions.** If you add a check that is supposed to
catch something, prove it catches it — deliberately break the thing and watch the
check fail. A guard only ever observed passing tells you nothing; the lockfile
guard above was written that way and would have looked fine either way.

---

## Adding a recognizer

The mechanics are in the README under *Adding a Tier A recognizer*. The
conventions that matter:

**Your score is a claim, so make it true.** `1.0` means a real checksum ran and
passed. `0.85`, `0.9`, `0.75` mean structural plausibility only. `eu_vat.rs`
reports `1.0` for the four countries whose check digits it verifies and `0.8` for
the rest, because claiming `1.0` for the others would assert a check that never
ran. Someone filtering on `--score-threshold 1.0` is trusting you.

**Reject a failed checksum outright — don't downgrade it.** Failing a check is
*stronger* evidence than not running one. A wrong checksum means it isn't the
thing; it doesn't mean "probably the thing".

**Check for collisions with the recognizers that already exist, and test it.**
`TierA::analyze` runs every recognizer independently with no overlap resolution,
so two recognizers can both claim the same characters. This has bitten already:
credit cards accepted any Luhn-valid 13–19 digits, which is *every French
SIRET* — so invoices had their legally-required company number blacked out and
reported as a `CREDIT_CARD`. The fix was requiring an allocated issuer prefix at
a length that network actually issues. When `EU_VAT` was added, IBANs were
explicitly checked for the same clash (they also open with a country code)
rather than assumed safe.

A checksum is a transcription check, not an identity check. Roughly one in ten
arbitrary strings of the right length passes Luhn. If your only filter is a
checksum, say what else anchors the match.

**Prefer a false positive to a miss, but say which you chose.** Diners Club cards
are 14 digits and genuinely overlap SIRET. They were kept, because dropping a
real card format to dodge a false positive is the wrong trade *for this tool*.
Make that call deliberately and write down which way you went and why.

---

## Comments and commit messages

Comments explain **why**, not what. The ones worth writing are the ones that stop
the next person from "simplifying" something load-bearing:

```rust
// Applied at the entry point rather than inside redact_image_bytes: rotating
// only at redaction time would draw boxes using coordinates from a
// differently-oriented OCR pass and put them in the wrong place.
```

Commit messages follow the same rule — they carry the reasoning, including what
was tried and rejected, what was measured, and who caught what. Wrong turns are
worth recording; they're what stops the same wrong turn next year. Look at
`git log` for the house style.

---

## Scope

**In scope:** recognizers, ingestion, redaction correctness, bindings, docs,
tests, performance.

**Out of scope:** deployment scripts, server configuration, hostnames, IPs, and
anything else operational. The public demo's deploy script is deliberately kept
out of version control. Please don't add one.

---

## Releasing (maintainers)

Use the script:

```bash
scripts/bump-version.sh 0.2.0
```

It updates every manifest — workspace `Cargo.toml`, the Node package and its five
per-platform packages, the Python `pyproject.toml` — regenerates `Cargo.lock`,
and then verifies the lock the same way `cargo publish --locked` will.

Do not bump versions by hand. Doing so broke two releases in the same way: the
manifests moved, the lockfile didn't, and `cargo publish --locked` refused it.
CI caught it the second time but couldn't prevent it, because CI runs on the
branch push while Publish runs on the tag, in parallel — the guard reported the
problem next to the failure instead of ahead of it. The script exists so there is
no order left to remember.

Then commit, tag `vX.Y.Z`, and push the tag. Publishing is driven entirely by the
tag.

**If a publish partially fails, check every registry before assuming it's fine.**
The jobs are independent: npm, PyPI and crates.io can and do disagree. One
release published to npm and PyPI but not crates.io; another published the root
npm package while the macOS ARM platform build failed, leaving Apple Silicon
users with a version that couldn't install. `npm view <pkg> version` per platform
package, not just the root.

---

## Security and privacy reports

If you find a way to make the tool return a document with PII still in it — a
document type that silently yields no detections, a coordinate bug that draws
boxes in the wrong place, an entity that slips past a recognizer that should
catch it — that is a **security issue**, not a feature request, and it is the
most valuable thing you can report.

Open an issue with a *synthetic* reproduction. Never attach the real document.

---

## License

By contributing you agree your contributions are licensed under the MIT License,
the same as the rest of the project.
