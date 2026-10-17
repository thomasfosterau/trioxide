// Generates src/options.d.ts and schema.json from the Rust-generated JSON schema
// (schema/trioxide.schema.json), so the TypeScript options type cannot drift from `Options`.
// `node scripts/generate-types.js --check` fails instead of writing when the files are stale.

import { readFileSync, writeFileSync } from 'node:fs';

const schemaPath = new URL('../../../schema/trioxide.schema.json', import.meta.url);
const typesPath = new URL('../src/options.d.ts', import.meta.url);
const schemaCopyPath = new URL('../schema.json', import.meta.url);

const schemaText = readFileSync(schemaPath, 'utf8');
const schema = JSON.parse(schemaText);

/** Rust doc text → TSDoc: drop intra-doc link brackets. */
function doc(description, indent, extra = []) {
  const lines = (description ?? '')
    .replace(/\[`([^`\]]+)`\](\([^)]*\))?/g, '`$1`')
    .replaceAll('*/', '*\\/')
    .split('\n')
    .concat(extra);
  const body = lines.filter((line, i) => line !== '' || i < lines.length - 1);
  if (body.length === 0 || (body.length === 1 && body[0] === '')) return '';
  return `${indent}/**\n${body.map((line) => `${indent} *${line ? ` ${line}` : ''}`).join('\n')}\n${indent} */\n`;
}

function typeOf(node) {
  if (node.$ref) return node.$ref.replace('#/$defs/', '');
  if (node.anyOf) return node.anyOf.map(typeOf).join(' | ');
  if (node.oneOf) return node.oneOf.map(typeOf).join(' | ');
  if ('const' in node) return JSON.stringify(node.const);
  if (node.enum) return node.enum.map((value) => JSON.stringify(value)).join(' | ');
  if (Array.isArray(node.type)) return node.type.map((type) => typeOf({ ...node, type })).join(' | ');
  switch (node.type) {
    case 'string':
      return 'string';
    case 'boolean':
      return 'boolean';
    case 'number':
    case 'integer':
      return 'number';
    case 'null':
      return 'null';
    case 'array':
      return `Array<${typeOf(node.items)}>`;
    case 'object':
      if (node.properties) return objectType(node, '');
      if (node.additionalProperties) return `Record<string, ${typeOf(node.additionalProperties)}>`;
      return 'Record<string, unknown>';
    default:
      throw new Error(`unsupported schema node: ${JSON.stringify(node)}`);
  }
}

function objectType(node, indent) {
  const required = new Set(node.required ?? []);
  const fields = Object.entries(node.properties).map(([key, property]) => {
    const extra = property.default === undefined ? [] : ['', `@default ${JSON.stringify(property.default)}`];
    const name = /^[A-Za-z_][A-Za-z0-9_]*$/.test(key) ? key : JSON.stringify(key);
    const optional = required.has(key) ? '' : '?';
    return `${doc(property.description, `${indent}  `, extra)}${indent}  ${name}${optional}: ${typeOf(property)};`;
  });
  return `{\n${fields.join('\n')}\n${indent}}`;
}

function declaration(name, node) {
  if (node.type === 'object' && node.properties) {
    return `${doc(node.description, '')}export interface ${name} ${objectType(node, '')}\n`;
  }
  if (node.oneOf) {
    const variants = node.oneOf.map((variant) => `${doc(variant.description, '  ')}  | ${typeOf(variant)}`);
    return `${doc(node.description, '')}export type ${name} =\n${variants.join('\n')};\n`;
  }
  return `${doc(node.description, '')}export type ${name} = ${typeOf(node)};\n`;
}

const parts = [
  '// Generated from schema/trioxide.schema.json by scripts/generate-types.js. Do not edit.\n',
  declaration('TrioxideOptions', schema),
  ...Object.entries(schema.$defs ?? {}).map(([name, node]) => declaration(name, node)),
];
const types = parts.join('\n');

if (process.argv.includes('--check')) {
  const stale = [
    [typesPath, types],
    [schemaCopyPath, schemaText],
  ].filter(([path, expected]) => {
    try {
      return readFileSync(path, 'utf8') !== expected;
    } catch {
      return true;
    }
  });
  if (stale.length > 0) {
    console.error(`stale: ${stale.map(([path]) => path.pathname).join(', ')}; run \`pnpm --filter trioxide generate\``);
    process.exit(1);
  }
} else {
  writeFileSync(typesPath, types);
  writeFileSync(schemaCopyPath, schemaText);
}
