/** Shared helpers of the project scripts: the root folder, `.env` and `services.json`. */
import { existsSync, readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { parseEnv } from 'node:util';

export const rootDir = resolve(import.meta.dirname, '../..');

export interface Service {
  name: string;
  package: string;
  portEnv: string;
  defaultPort: number;
}

/** Every service of `services.json`, in file order. */
export function readServices(): Service[] {
  const registry = JSON.parse(readFileSync(join(rootDir, 'services.json'), 'utf8')) as {
    services: Service[];
  };
  return registry.services;
}

/** Stops the script with a hint when `.env` is missing. */
export function requireEnvFile(): void {
  if (!existsSync(join(rootDir, '.env'))) {
    console.error('Missing .env in the project root. Create it with: npm run setup');
    process.exit(1);
  }
}

/** Values of the root `.env` (read like dotenv, no `$VAR` expansion); the environment wins. */
export function readEnv(): Record<string, string | undefined> {
  const file = join(rootDir, '.env');
  const values = existsSync(file) ? parseEnv(readFileSync(file, 'utf8')) : {};
  return { ...values, ...process.env };
}

/**
 * glibc malloc settings for the Rust processes (also in the Dockerfile): without them glibc keeps
 * what it freed, and every login (argon2, 19 MB) leaves the app bigger (docs/bridges.md). glibc
 * reads them from the real environment only, so the runners pass them on; a value in `.env` or the
 * environment wins. Other allocators (macOS) ignore them.
 */
export const MALLOC_DEFAULTS = {
  MALLOC_ARENA_MAX: '2',
  MALLOC_MMAP_THRESHOLD_: '1048576',
  MALLOC_TRIM_THRESHOLD_: '1048576',
} as const;

/** Puts the malloc settings into `process.env`, so every child (or the execve'd app) gets them. */
export function applyMallocSettings(env: Record<string, string | undefined>): void {
  for (const [key, value] of Object.entries(MALLOC_DEFAULTS)) {
    process.env[key] = env[key] || value;
  }
}
