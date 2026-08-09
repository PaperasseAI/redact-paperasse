import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

import { redactText, redactImage, redactPdf } from './index.js';

const __dirname = dirname(fileURLToPath(import.meta.url));

const text =
  "Bonjour, je m'appelle Jean Dupont. Mon email est jean.dupont@example.com " +
  'et mon numéro de sécurité sociale est 2 91 05 99 338 076 92. Merci de traiter ma demande.';

console.log('--- input ---');
console.log(text);

const redacted = await redactText(text);
console.log('\n--- redacted (Native) ---');
console.log(redacted);

const redactedMarkdown = await redactText(text, { markdown: true });
console.log('\n--- redacted (Markdown) ---');
console.log(redactedMarkdown);

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
