/**
 * `npm start`: runs the release builds (`npm run build` first). The edge serves the built web app
 * (web/dist) and the API on EDGE_PORT.
 *
 * By default all services run inside one process, target/release/app-all, and this script turns
 * into it (execve), so no Node stays in memory. With BRIDGES=off or SEPARATE_SERVICES
 * (docs/bridges.md) there are several processes: it stays as their supervisor, with prefixed
 * output, restarts a crashed one after a pause and stops all of them on SIGINT/SIGTERM.
 *
 *   node scripts/start.ts                      as .env says (the smallest: npm itself stays out)
 *   node scripts/start.ts --only notes,edge    these services, each in its own process
 *   node scripts/start.ts --check              check .env and the binaries, print the plan, exit
 */
import { existsSync } from 'node:fs';
import { join, relative } from 'node:path';
import { parseArgs } from 'node:util';
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
  options: { only: { type: 'string' }, check: { type: 'boolean', default: false } },
});
requireEnvFile();
const env = readEnv();
applyMallocSettings(env);
const services = readServices();
const only = values.only?.split(',').map((name) => name.trim());
const unknown = only?.filter((name) => !services.some((s) => s.name === name)) ?? [];
if (unknown.length > 0) {
  console.error(`Unknown service(s): ${unknown.join(', ')} (see services.json)`);
  process.exit(1);
}

let processes: Process[];
try {
  processes = only
    ? services.filter((service) => only.includes(service.name))
    : processesOf(services, env);
} catch (error) {
  console.error(error instanceof Error ? error.message : error);
  process.exit(1);
}

const binaryOf = (p: Process) => join(rootDir, 'target/release', p.package);
const missing = processes.filter((p) => !existsSync(binaryOf(p)));
if (missing.length > 0 || !existsSync(join(rootDir, 'web/dist/index.html'))) {
  const list = missing.map((p) => relative(rootDir, binaryOf(p))).join(', ');
  console.error(`Release build not found${list ? ` (${list})` : ''}. Build it with: npm run build`);
  process.exit(1);
}
if (values.check) {
  for (const p of processes)
    console.log(`ok  ${p.name.padEnd(10)} ${relative(rootDir, binaryOf(p))}`);
  process.exit(0);
}

const edge = services.find((service) => service.name === 'edge');
const port = edge ? (env[edge.portEnv] ?? String(edge.defaultPort)) : '';
console.log(`Open http://localhost:${port}`);

// One process needs no supervisor: Docker or a process manager restarts it after a crash.
const [alone] = processes;
if (alone && processes.length === 1 && typeof process.execve === 'function') {
  process.chdir(rootDir);
  process.execve(binaryOf(alone), [binaryOf(alone)], process.env);
}

const tasks: Task[] = processes.map((p) => ({
  name: p.name,
  command: binaryOf(p),
  args: [],
  cwd: rootDir,
  kind: 'rust',
}));
new Runner().startAll(tasks);
