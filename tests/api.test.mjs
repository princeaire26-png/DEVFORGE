import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const tsxCli = path.join(root, 'node_modules', 'tsx', 'dist', 'cli.mjs');

async function unusedPort() {
  const server = net.createServer();
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const { port } = server.address();
  await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  return port;
}

async function withServer(mode, callback) {
  const directory = await mkdtemp(path.join(os.tmpdir(), 'devforge-api-test-'));
  const port = await unusedPort();
  const child = spawn(process.execPath, [tsxCli, 'server/index.ts'], {
    cwd: root,
    env: { ...process.env, PORT: String(port), DEVFORGE_DB: path.join(directory, 'test.db'), DEVFORGE_AI_MODE: mode },
    stdio: 'ignore',
  });
  const base = `http://127.0.0.1:${port}`;
  try {
    let ready = false;
    for (let attempt = 0; attempt < 100; attempt += 1) {
      if (child.exitCode !== null) throw new Error(`API server exited before startup (${child.exitCode}).`);
      try {
        const response = await fetch(`${base}/api/health`);
        if (response.ok) { ready = true; break; }
      } catch {}
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    assert.equal(ready, true, 'API server should start within five seconds');
    await callback(base);
  } finally {
    const exited = child.exitCode !== null ? Promise.resolve() : new Promise(resolve => child.once('exit', resolve));
    child.kill();
    await Promise.race([exited, new Promise(resolve => setTimeout(resolve, 1_000))]);
    if (child.exitCode === null) {
      child.kill('SIGKILL');
      await exited;
    }
    await rm(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  }
}

test('API starts and labels its deterministic blueprint as demo', async () => {
  await withServer('demo', async base => {
    const health = await fetch(`${base}/api/health`);
    assert.deepEqual(await health.json(), { ok: true, service: 'DEVFORGE', version: '1.5.1' });
    const response = await fetch(`${base}/api/projects/plan`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ idea: 'A school attendance dashboard' }),
    });
    const plan = await response.json();
    assert.equal(response.status, 200);
    assert.equal(plan.mode, 'demo');
    assert.equal(plan.plan.coreFeatures.includes('Attendance'), true);
  });
});

test('API does not label an unavailable provider as AI-generated', async () => {
  await withServer('replit', async base => {
    const response = await fetch(`${base}/api/projects/plan`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ idea: 'A school attendance dashboard' }),
    });
    const result = await response.json();
    assert.equal(response.status, 503);
    assert.match(result.error, /no configured AI provider/i);
  });
});
