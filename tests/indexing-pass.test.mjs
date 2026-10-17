import assert from 'node:assert/strict';
import test from 'node:test';

const { formatIndexingTime, indexingPassSummary, indexingPassStages } = await import(
  new URL('../src/lib/utils/indexing-pass.ts', import.meta.url)
);

const pass = {
  notes: 3,
  chunks: 4,
  totalMs: 83_400,
  readMs: 120,
  chunkMs: 40,
  embedMs: 79_950,
  databaseMs: 2_300,
  stopped: false
};

test('shows times under a minute in seconds with one decimal', () => {
  assert.equal(formatIndexingTime(0), '0.0 s');
  assert.equal(formatIndexingTime(40), '0.0 s');
  assert.equal(formatIndexingTime(50), '0.1 s');
  assert.equal(formatIndexingTime(1_234), '1.2 s');
  assert.equal(formatIndexingTime(59_949), '59.9 s');
});

test('shows a minute and longer as minutes and seconds', () => {
  assert.equal(formatIndexingTime(60_000), '1 min 0 s');
  assert.equal(formatIndexingTime(83_400), '1 min 23 s');
  assert.equal(formatIndexingTime(3_725_000), '62 min 5 s');
});

test('never shows a time that rounds up to a minute as 60 seconds', () => {
  assert.equal(formatIndexingTime(59_960), '1 min 0 s');
  assert.equal(formatIndexingTime(119_600), '2 min 0 s');
});

test('summarises a pass as its notes, chunks and total time', () => {
  assert.equal(indexingPassSummary(pass), '3 notes, 4 chunks in 1 min 23 s');
  assert.equal(
    indexingPassSummary({ ...pass, notes: 1, chunks: 1, totalMs: 900 }),
    '1 note, 1 chunk in 0.9 s'
  );
  assert.equal(
    indexingPassSummary({ ...pass, notes: 0, chunks: 0, totalMs: 15, stopped: true }),
    '0 notes, 0 chunks in 0.0 s'
  );
});

test('labels the four stage times in order', () => {
  assert.deepEqual(indexingPassStages(pass), [
    { label: 'File reads', time: '0.1 s' },
    { label: 'Chunking', time: '0.0 s' },
    { label: 'Ollama', time: '1 min 20 s' },
    { label: 'Database', time: '2.3 s' }
  ]);
});
