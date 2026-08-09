import { redactText } from './index.js';

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
