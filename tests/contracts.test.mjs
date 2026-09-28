import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import ts from 'typescript';

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');
const rustTypes = read('src-tauri/src/types.rs');
const tsTypes = ts.createSourceFile('types.ts', read('src/lib/types.ts'), ts.ScriptTarget.Latest, true);

function rustFields(name) {
  const body = rustTypes.match(new RegExp(`pub struct ${name} \\{([\\s\\S]*?)\\n\\}`))?.[1];
  assert.ok(body, `Rust ${name} missing`);
  return [...body.matchAll(/\bpub (\w+):/g)].map((match) => match[1]);
}

function tsFields(name) {
  const node = tsTypes.statements.find((statement) => ts.isInterfaceDeclaration(statement) && statement.name.text === name);
  assert.ok(node, `TypeScript ${name} missing`);
  return node.members.map((member) => member.name.getText(tsTypes));
}

test('persisted Rust and TypeScript models expose the same fields', () => {
  for (const name of ['AppConfig', 'VaultConfig', 'VaultState']) {
    const rust = rustFields(name).filter((field) => !['legacy_vaults', 'config_save_blocked'].includes(field));
    assert.deepEqual(tsFields(name).sort(), rust.sort(), `${name} fields diverged`);
  }
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
