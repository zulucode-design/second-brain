#!/usr/bin/env node
// Cold-start harness for the issue #88 startup budget.
//
//   node scripts/perf-startup.mjs <installed-app-executable> <samples.jsonl> [runs=5]
//
// Each run appends a `launch` record, starts the app with the performance probe enabled, and
// waits for the app to report `startup-ready` and exit through its normal shutdown path. The
// app must already be configured with the fixture vault and a restored last note. A run that
// does not exit within 60 s is stopped by the exact PID this script launched.

import { spawn } from 'node:child_process';
import { appendFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

const [exe, log, runsArg = '5'] = process.argv.slice(2);
if (!exe || !log) {
  console.error('usage: perf-startup.mjs <app-executable> <samples.jsonl> [runs]');
  process.exit(2);
}
const logPath = resolve(log);
const env = { ...process.env, SECOND_BRAIN_PERF_LOG: logPath, SECOND_BRAIN_PERF_EXIT_AFTER_STARTUP: '1' };

for (let run = 1; run <= Number(runsArg); run += 1) {
  appendFileSync(logPath, `${JSON.stringify({ kind: 'launch', run, epochMs: Date.now() })}\n`);
  const child = spawn(exe, [], { env, stdio: 'ignore' });
  const timer = setTimeout(() => {
    appendFileSync(logPath, `${JSON.stringify({ kind: 'launch-timeout', run, pid: child.pid, epochMs: Date.now() })}\n`);
    child.kill();
  }, 60_000);
  const code = await new Promise((done) => {
    child.on('exit', done);
    child.on('error', (error) => { console.error(error.message); done(-1); });
  });
  clearTimeout(timer);
  console.log(`run ${run}: exit ${code}`);
  await sleep(5000);
}
