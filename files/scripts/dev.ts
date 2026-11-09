/**
 * `npm run dev`: builds the Rust workspace, starts the services and the Vite dev server with
 * prefixed output. By default the services run in one process, `app-all`; BRIDGES=off and
 * SEPARATE_SERVICES in .env move them into processes of their own (docs/bridges.md). When Rust
 * sources, migrations, services.json or .env change it rebuilds and restarts the services (the web
 * app hot-reloads by itself).
 *
 *   npm run dev                         as .env says + web
 *   npm run dev -- --only notes,edge    these services, each in its own process (+ web)
 *   npm run dev -- --no-web             services only
 */
import { spawnSync } from 'node:child_process';
import { watch } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { parseArgs, styleText } from 'node:util';
import {
  applyMallocSettings,
  readEnv,
  readServices,
  requireEnvFile,
  rootDir,
} from './lib/project.ts';
import { Runner, type Task } from './lib/runner.ts';
import { type Process, processesOf } from './lib/topology.ts';

const { values } = parseArgs({
  options: { only: { type: 'string' }, 'no-web': { type: 'boolean', default: false } },
});
requireEnvFile();
const env = readEnv();
applyMallocSettings(env);

const only = values.only?.split(',').map((name) => name.trim());
const services = readServices();
let processes: Process[];
try {
  processes = only
    ? services.filter((service) => only.includes(service.name))
    : processesOf(services, env);
} catch (error) {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
}

function build(): boolean {
  const packages = processes.flatMap((p) => ['-p', p.package]);
  return (
    spawnSync('cargo', ['build', ...packages], { cwd: rootDir, stdio: 'inherit' }).status === 0
  );
}

const tasks: Task[] = processes.map((p) => ({
  name: p.name,
  command: join(rootDir, 'target/debug', p.package),
  args: [],
  cwd: rootDir,
  kind: 'rust',
}));
if (!values['no-web']) {
  const vitePackage = createRequire(join(rootDir, 'web/package.json')).resolve('vite/package.json');
  tasks.push({
    name: 'web',
    command: process.execPath,
    args: [join(dirname(vitePackage), 'bin/vite.js')],
    cwd: join(rootDir, 'web'),
    kind: 'web',
  });
}

/** Whether `.env` now asks for other processes than the ones running. */
function layoutChanged(): boolean {
  try {
    const now = processesOf(services, readEnv()).map((p) => p.name);
    return now.join() !== processes.map((p) => p.name).join();
  } catch {
    return true;
  }
}

/** Rebuilds and restarts the services when Rust sources, migrations, services.json or .env change. */
function watchRust(runner: Runner): void {
  let timer: NodeJS.Timeout | undefined;
  const schedule = (file: string | null) => {
    if (runner.isStopping || !file || file.includes('target')) return;
    if (!/\.(rs|toml|sql|json)$|^\.env$/.test(file)) return;
    clearTimeout(timer);
    timer = setTimeout(() => {
      if (file === '.env' && !only && layoutChanged()) {
        console.log(
          styleText(
            'yellow',
            'BRIDGES or SEPARATE_SERVICES changed: restart `npm run dev` to apply',
          ),
        );
      }
      console.log(styleText('gray', `change in ${file}: rebuilding…`));
      if (build()) runner.restart('rust');
      else console.log(styleText('red', 'build failed: services keep running the previous build'));
    }, 400);
  };
  for (const dir of ['app', 'crates', 'services', 'migrations']) {
    watch(join(rootDir, dir), { recursive: true }, (_event, file) => schedule(file));
  }
  watch(rootDir, (_event, file) => {
    if (file === 'services.json' || file === '.env' || file === 'Cargo.toml') schedule(file);
  });
}

if (!build()) {
  console.error('cargo build failed');
  process.exit(1);
}
const runner = new Runner();
runner.startAll(tasks);
watchRust(runner);
