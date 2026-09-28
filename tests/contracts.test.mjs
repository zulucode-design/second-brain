import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import ts from 'typescript';

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');
const rustTypes = read('src-tauri/src/types.rs');
const notionTypes = read('src-tauri/src/notion/config.rs');
const tsTypes = ts.createSourceFile('types.ts', read('src/lib/types.ts'), ts.ScriptTarget.Latest, true);

function rustStruct(name, source = rustTypes) {
  const body = source.match(new RegExp(`pub struct ${name} \\{([\\s\\S]*?)\\n\\}`))?.[1];
  assert.ok(body, `Rust ${name} missing`);
  return body;
}

function rustFields(name, source = rustTypes) {
  return Object.fromEntries([...rustStruct(name, source).matchAll(/^\s*pub (\w+): (.+),$/gm)].map(([, field, type]) => [field, type]));
}

function rustOptionalFields(name) {
  return [...rustStruct(name).matchAll(/#\[serde\([^\]]*skip_serializing_if[^\]]*\)\]\s*pub (\w+):/g)].map((match) => match[1]).sort();
}

function tsInterface(name) {
  const node = tsTypes.statements.find((statement) => ts.isInterfaceDeclaration(statement) && statement.name.text === name);
  assert.ok(node, `TypeScript ${name} missing`);
  return node;
}

function tsFields(name) {
  return Object.fromEntries(tsInterface(name).members.map((member) => [member.name.getText(tsTypes), member.type]));
}

function tsOptionalFields(name) {
  return tsInterface(name).members.filter((member) => member.questionToken).map((member) => member.name.getText(tsTypes)).sort();
}

function rustShape(type) {
  if (type.startsWith('Option<')) return `${rustShape(type.slice(7, -1))}|null`;
  if (type.startsWith('Vec<')) return `${rustShape(type.slice(4, -1))}[]`;
  if (type.startsWith('std::collections::HashMap<')) {
    const [key, value] = type.slice(26, -1).split(', ');
    return `Record<${rustShape(key)},${rustShape(value)}>`;
  }
  if (type === 'String') return 'string';
  if (type === 'bool') return 'boolean';
  if (/^(u|i|f)\d+$/.test(type)) return 'number';
  if (type === 'AiProvider') {
    assert.match(rustTypes, /Unknown\(String\)/);
    assert.match(rustTypes, /serialize_str\(self\.id\(\)\)/);
    return 'string';
  }
  if (['VaultConfig', 'CustomTheme', 'CustomThemeColors', 'StartupView'].includes(type)) return type;
  assert.fail(`Unmapped persisted Rust type: ${type}`);
}

function tsShape(type) {
  if (ts.isUnionTypeNode(type)) return type.types.map(tsShape).sort().join('|');
  if (ts.isLiteralTypeNode(type) && type.literal.kind === ts.SyntaxKind.NullKeyword) return 'null';
  if (ts.isArrayTypeNode(type)) return `${tsShape(type.elementType)}[]`;
  if (ts.isTypeReferenceNode(type)) return `${type.typeName.getText(tsTypes)}${type.typeArguments?.length ? `<${type.typeArguments.map(tsShape).join(',')}>` : ''}`;
  const primitives = { [ts.SyntaxKind.StringKeyword]: 'string', [ts.SyntaxKind.NumberKeyword]: 'number', [ts.SyntaxKind.BooleanKeyword]: 'boolean', [ts.SyntaxKind.NullKeyword]: 'null' };
  if (primitives[type.kind]) return primitives[type.kind];
  assert.fail(`Unmapped persisted TypeScript type: ${type.getText(tsTypes)}`);
}

function tsLiterals(name) {
  const alias = tsTypes.statements.find((statement) => ts.isTypeAliasDeclaration(statement) && statement.name.text === name);
  assert.ok(alias && ts.isUnionTypeNode(alias.type), `TypeScript ${name} union missing`);
  return alias.type.types.map((type) => {
    assert.ok(ts.isLiteralTypeNode(type) && ts.isStringLiteral(type.literal));
    return type.literal.text;
  }).sort();
}

test('persisted Rust and TypeScript models retain field types and nullability', () => {
  for (const name of ['AppConfig', 'VaultConfig', 'VaultState', 'CustomTheme', 'CustomThemeColors']) {
    const rust = rustFields(name);
    delete rust.legacy_vaults;
    delete rust.config_save_blocked;
    delete rust.legacy_move_incomplete;
    const frontend = tsFields(name);
    assert.deepEqual(Object.keys(frontend).sort(), Object.keys(rust).sort(), `${name} fields diverged`);
    assert.deepEqual(tsOptionalFields(name), rustOptionalFields(name), `${name} optional fields diverged`);
    for (const [field, type] of Object.entries(rust)) {
      if (name === 'VaultConfig' && field === 'notion') continue;
      assert.equal(tsShape(frontend[field]), rustShape(type).split('|').sort().join('|'), `${name}.${field} type diverged`);
    }
  }
  const notion = rustFields('NotionSettings', notionTypes);
  assert.equal(rustFields('VaultConfig').notion, 'crate::notion::config::NotionSettings');
  const frontendNotion = tsFields('VaultConfig').notion;
  assert.ok(ts.isTypeLiteralNode(frontendNotion));
  const fields = Object.fromEntries(frontendNotion.members.map((member) => [member.name.getText(tsTypes), member.type]));
  assert.deepEqual(Object.keys(fields).sort(), Object.keys(notion).sort(), 'Notion settings fields diverged');
  for (const [field, type] of Object.entries(notion)) {
    assert.equal(tsShape(fields[field]), rustShape(type).split('|').sort().join('|'), `NotionSettings.${field} type diverged`);
  }
  const startupVariants = rustTypes.match(/pub enum StartupView \{([\s\S]*?)\n\}/)?.[1];
  assert.ok(startupVariants);
  assert.deepEqual(tsLiterals('StartupView'), [...startupVariants.matchAll(/^\s*(\w+),$/gm)].map((match) => match[1].toLowerCase()).sort());
  const rustProviders = [...rustTypes.matchAll(/Self::\w+ => "([^"]+)"/g)].map((match) => match[1]);
  assert.deepEqual(tsLiterals('AiProvider'), [...new Set(rustProviders)].sort());
});

test('typed frontend events match every backend wire name', () => {
  const rust = read('src-tauri/src/events.rs');
  const frontend = ts.createSourceFile('events.ts', read('src/lib/events.ts'), ts.ScriptTarget.Latest, true);
  const declaration = frontend.statements
    .filter(ts.isVariableStatement)
    .flatMap((statement) => [...statement.declarationList.declarations])
    .find((item) => item.name.getText(frontend) === 'EVENTS');
  assert.ok(declaration && ts.isAsExpression(declaration.initializer));
  const names = declaration.initializer.expression.properties.map((item) => ({
    key: item.name.getText(frontend),
    wire: item.initializer.text,
  }));
  const payloads = frontend.statements.find((statement) => ts.isInterfaceDeclaration(statement) && statement.name.text === 'EventPayloads');
  assert.ok(payloads);
  assert.deepEqual(payloads.members.map((member) => member.name.getText(frontend)).sort(), names.map((item) => item.key).sort());
  const backend = [...rust.matchAll(/pub const (\w+): &str = "([a-z-]+)";/g)]
    .map((match) => ({ key: match[1], wire: match[2] }));
  assert.deepEqual(backend.sort((a, b) => a.wire.localeCompare(b.wire)),
    names.map(({ wire }) => ({ key: wire.toUpperCase().replaceAll('-', '_'), wire }))
      .sort((a, b) => a.wire.localeCompare(b.wire)));
});
