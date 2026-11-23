/**
 * `npm run walls`: the quality gate, `walls check` (ignored paths in walls.json, project walls in
 * .walls/rules.json), run before `npm run security` in `npm run check`. Passes its exit code
 * through; extra arguments go to walls (`npm run walls -- --json`).
 *
 * Without the `walls` command it fails with the install hint, CI included, so a missing gate is
 * never mistaken for a passed one. Only `WALLS_OPTIONAL=1`, set where walls cannot be
 * installed, turns that into a pass with a loud notice: run the step elsewhere.
 */
import { spawnSync } from 'node:child_process';
import { rootDir } from './lib/project.ts';

// No shell: the arguments are an array and reach walls as they are.
const result = spawnSync('walls', ['check', ...process.argv.slice(2)], {
  cwd: rootDir,
  stdio: 'inherit',
});

if (result.error) {
  if ((result.error as NodeJS.ErrnoException).code !== 'ENOENT') throw result.error;
  if (process.env.WALLS_OPTIONAL === '1') {
    // In GitHub Actions the notice becomes a warning annotation on the run's summary page.
    const prefix = process.env.GITHUB_ACTIONS === 'true' ? '::warning::' : '';
    console.warn(
      `${prefix}walls skipped: not installed; run \`npm run walls\` on a machine with walls`,
    );
    process.exit(0);
  }
  console.error('walls is not installed: run "npm link" in ~/Documents/walls');
  process.exit(1);
}
process.exit(result.status ?? 1);
