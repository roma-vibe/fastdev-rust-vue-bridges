/**
 * What runs as which process (docs/bridges.md): `app-all` with every bridged service, plus a
 * process per service that runs separately. The same rule as `Config::separate_services` in
 * crates/core/src/config.rs (and `app-all --plan`, used by Docker); keep them in step.
 */
import type { Service } from './project.ts';

/** The binary that hosts the bridged services in one process. */
export const APP = { name: 'app', package: 'app-all' } as const;

export type Process = Pick<Service, 'name' | 'package'>;
export type Env = Record<string, string | undefined>;

/**
 * The processes to run for `services` (all of `services.json`).
 * @throws on an invalid `BRIDGES` or an unknown name in `SEPARATE_SERVICES`.
 */
export function processesOf(services: Process[], env: Env): Process[] {
  const bridges = (env.BRIDGES || 'on').toLowerCase();
  if (!['on', 'true', '1', 'off', 'false', '0'].includes(bridges)) {
    throw new Error(`BRIDGES must be on or off, got "${env.BRIDGES}"`);
  }
  const separate = (env.SEPARATE_SERVICES ?? '')
    .split(',')
    .map((name) => name.trim())
    .filter(Boolean);
  const unknown = separate.filter((name) => !services.some((s) => s.name === name));
  if (unknown.length > 0) {
    throw new Error(
      `SEPARATE_SERVICES: unknown service(s) ${unknown.join(', ')} (see services.json)`,
    );
  }
  if (['off', 'false', '0'].includes(bridges)) return services;
  const own = services.filter((s) => separate.includes(s.name));
  return own.length === services.length ? own : [APP, ...own];
}
