// Hand-written stub. `napi build` (see package.json's "build" script)
// generates the real, complete .d.ts from the #[napi] annotations in
// src/lib.rs — this file exists so the package is usable before a native
// build has been run once.

/**
 * Redact PII from plain text (Tier A: in-process regex+checksum
 * recognizers, no network call). Pass `markdown: true` to force markdown
 * output.
 */
export function redactText(text: string, markdown?: boolean): Promise<string>;
