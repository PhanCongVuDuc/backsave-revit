// Builds the server, the engine stub and the web app once, then runs the real server on the built app. Started by
// playwright.config.ts; it stays in the foreground so Playwright can stop it.
import { type ChildProcess, spawn, spawnSync } from 'node:child_process';
import { mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const webDir = resolve(import.meta.dirname, '..');
const repoDir = resolve(webDir, '..');
const exe = process.platform === 'win32' ? '.exe' : '';
const shell = process.platform === 'win32';

function run(command: string, args: string[], cwd: string) {
  const result = spawnSync(command, args, { cwd, stdio: 'inherit', shell });
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

run('cargo', ['build', '-p', 'backsave-server', '-p', 'engine-stub'], repoDir);
run('npx', ['vite', 'build'], webDir);

const dataDir = join(tmpdir(), 'backsave-e2e-data');
rmSync(dataDir, { recursive: true, force: true });
mkdirSync(dataDir, { recursive: true });

const server: ChildProcess = spawn(join(repoDir, 'target', 'debug', `backsave-server${exe}`), [], {
  cwd: repoDir,
  stdio: 'inherit',
  env: {
    ...process.env,
    BACKSAVE_ENGINE: join(repoDir, 'target', 'debug', `engine-stub${exe}`),
    BACKSAVE_DATA: dataDir,
    BACKSAVE_WEB_DIR: join(webDir, 'dist'),
    BACKSAVE_LISTEN: '127.0.0.1:8181',
    BACKSAVE_TRUST_PROXY: 'true',
  },
});

function stop() {
  server.kill();
}
process.on('SIGTERM', stop);
process.on('SIGINT', stop);
process.on('exit', stop);
server.on('exit', (code) => {
  rmSync(dataDir, { recursive: true, force: true });
  process.exit(code ?? 0);
});
