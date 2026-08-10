#!/usr/bin/env python3
"""Bundle libpdfium next to the compiled extension inside a maturin-built wheel.

Why this exists: liteparse-pdfium loads PDFium at runtime via dlopen, not at
link time, so maturin's automatic shared-library bundling (which inspects
link-time dependencies via otool/ldd) never sees it, and a plain `maturin
build` produces a wheel that panics on first use of redact_image/redact_pdf
on any machine other than the one it was built on -- found by actually
installing the published v0.1.0 wheel from PyPI into a fresh venv and running
it, not by reasoning about the build.

liteparse-pdfium's own search order (crates/liteparse-pdfium-sys/src/dynamic.rs)
includes "next to the native extension module" via dladdr/GetModuleHandleExW,
so bundling the library there is enough -- no runtime code changes needed on
our side, just packaging.

Usage: bundle_pdfium_into_wheel.py <wheel_path> <pdfium_lib_path> <package_dir>
  wheel_path      path to the .whl file to patch, in place
  pdfium_lib_path path to the platform's libpdfium.dylib / .so / pdfium.dll,
                  as already copied to target/<profile>/deps/ by
                  liteparse-pdfium-sys's build.rs
  package_dir     the wheel-internal directory the compiled extension lives
                  in (e.g. "redact_paperasse") -- verified by inspecting a
                  real built wheel's contents, not assumed
"""

import base64
import hashlib
import sys
import zipfile
from pathlib import Path


def record_line(arcname: str, data: bytes) -> str:
    digest = hashlib.sha256(data).digest()
    b64 = base64.urlsafe_b64encode(digest).rstrip(b"=").decode("ascii")
    return f"{arcname},sha256={b64},{len(data)}"


def main() -> None:
    if len(sys.argv) != 4:
        print(__doc__, file=sys.stderr)
        sys.exit(1)

    wheel_path = Path(sys.argv[1])
    pdfium_lib_path = Path(sys.argv[2])
    package_dir = sys.argv[3]

    if not wheel_path.is_file():
        sys.exit(f"error: wheel not found at {wheel_path}")
    if not pdfium_lib_path.is_file():
        sys.exit(f"error: pdfium library not found at {pdfium_lib_path}")

    pdfium_data = pdfium_lib_path.read_bytes()
    arcname = f"{package_dir}/{pdfium_lib_path.name}"

    with zipfile.ZipFile(wheel_path, "r") as z:
        names = z.namelist()
    if arcname in names:
        sys.exit(f"error: {arcname} already present in {wheel_path.name} -- refusing to overwrite")
    record_names = [n for n in names if n.endswith(".dist-info/RECORD")]
    if len(record_names) != 1:
        sys.exit(f"error: expected exactly one RECORD file, found {record_names}")
    record_name = record_names[0]

    with zipfile.ZipFile(wheel_path, "r") as z:
        record_text = z.read(record_name).decode("utf-8")

    new_line = record_line(arcname, pdfium_data)
    # RECORD's own entry has no hash/size (it can't hash itself), so it's
    # always the last real line -- insert before it, not just append, to
    # keep that convention intact.
    lines = record_text.splitlines()
    self_entry = [l for l in lines if l.startswith(f"{record_name},")]
    other_lines = [l for l in lines if not l.startswith(f"{record_name},")]
    new_record_text = "\n".join(other_lines + [new_line] + self_entry) + "\n"

    with zipfile.ZipFile(wheel_path, "a") as z:
        z.write(pdfium_lib_path, arcname)

    # ZipFile has no in-place remove/replace -- rewrite the archive to
    # actually replace RECORD's content rather than leaving a stale
    # duplicate entry (a zip can contain two entries with the same name;
    # most readers use the last one, but that's fragile to depend on).
    tmp_path = wheel_path.with_suffix(".whl.tmp")
    with zipfile.ZipFile(wheel_path, "r") as src, zipfile.ZipFile(
        tmp_path, "w", zipfile.ZIP_DEFLATED
    ) as dst:
        for item in src.infolist():
            if item.filename == record_name:
                continue
            dst.writestr(item, src.read(item.filename))
        dst.writestr(record_name, new_record_text)
    tmp_path.replace(wheel_path)

    print(f"bundled {pdfium_lib_path.name} into {wheel_path.name} at {arcname}")


if __name__ == "__main__":
    main()
