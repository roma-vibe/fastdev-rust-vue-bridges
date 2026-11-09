import { describe, expect, it } from 'vitest';
import { readServices } from './project.ts';
import { APP, processesOf } from './topology.ts';

const services = readServices();
const names = (env: Record<string, string>) => processesOf(services, env).map((p) => p.name);

describe('processesOf', () => {
  it('runs everything in app-all by default', () => {
    expect(processesOf(services, {})).toEqual([APP]);
    expect(names({ BRIDGES: 'on' })).toEqual(['app']);
  });

  it('keeps separate services in processes of their own', () => {
    expect(names({ SEPARATE_SERVICES: 'notes' })).toEqual(['app', 'notes']);
    expect(names({ SEPARATE_SERVICES: ' notes , ' })).toEqual(['app', 'notes']);
  });

  it('runs every service alone with BRIDGES=off', () => {
    expect(names({ BRIDGES: 'off', SEPARATE_SERVICES: 'notes' })).toEqual(
      services.map((s) => s.name),
    );
  });

  it('rejects wrong settings', () => {
    expect(() => names({ BRIDGES: 'maybe' })).toThrow(/BRIDGES/);
    expect(() => names({ SEPARATE_SERVICES: 'nope' })).toThrow(/nope/);
  });
});
