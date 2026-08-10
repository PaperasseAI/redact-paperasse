import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

import { redactText, redactImage, redactPdf } from './index.js';

const __dirname = dirname(fileURLToPath(import.meta.url));

const text =
  "Bonjour, je m'appelle Jean Dupont. Mon email est jean.dupont@example.com " +
  'et mon numéro de sécurité sociale est 1 85 01 75 123 456 09. Merci de traiter ma demande.';

console.log('--- input ---');
console.log(text);

// Markdown is the default output — this is a tool for agents, and markdown
// is what they parse best.
const redactedMarkdown = await redactText(text);
console.log('\n--- redacted (Markdown, the default) ---');
console.log(redactedMarkdown);

// { markdown: false } opts back into plain text, in the input's own shape.
const redacted = await redactText(text, { markdown: false });
console.log('\n--- redacted (Native / plain text) ---');
console.log(redacted);

// entities filter — matches Presidio's analyzer_entities: redact ONLY the
// NIR, leave the email (and the name — Tier A has no NER) untouched.
const nirOnly = await redactText(text, { entities: ['FR_NIR'] });
console.log('\n--- redacted (entities: ["FR_NIR"] only) ---');
console.log(nirOnly);

// Pixel redaction — a small synthetic fixture (not a real photo/document),
// same content as the text example above, so we can assert the NIR is
// actually gone from the output bytes without needing a real test image.
console.log('\n--- redactImage (fixtures/sample.png) ---');
const imageBytes = await readFile(join(__dirname, 'fixtures', 'sample.png'));
const redactedImage = await redactImage(imageBytes);
await writeFile(join(__dirname, 'fixtures', 'sample.redacted.png'), redactedImage);
console.log(`${imageBytes.length} bytes in -> ${redactedImage.length} bytes out (wrote fixtures/sample.redacted.png)`);

console.log('\n--- redactPdf (fixtures/sample.pdf) ---');
const pdfBytes = await readFile(join(__dirname, 'fixtures', 'sample.pdf'));
const redactedPdf = await redactPdf(pdfBytes);
await writeFile(join(__dirname, 'fixtures', 'sample.redacted.pdf'), redactedPdf);
console.log(`${pdfBytes.length} bytes in -> ${redactedPdf.length} bytes out (wrote fixtures/sample.redacted.pdf)`);
