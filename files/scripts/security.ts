/**
 * `npm run security`: the security gate, `secbase check` (profile and exceptions in secbase.json),
 * the last step of `npm run check`. Passes its exit code through.
 *
 * Without the `secbase` command it fails with the install hint, CI included, so a missing gate is
 * never mistaken for a passed one. Only `SECBASE_OPTIONAL=1`, set where secbase cannot be
 * installed, turns that into a pass with a loud notice: run the step elsewhere.
 */
import { spawnSync } from 'node:child_process';
import { rootDir } from './lib/project.ts';

const result = spawnSync('secbase', ['check'], { cwd: rootDir, stdio: 'inherit' });

if (result.error) {
  if ((result.error as NodeJS.ErrnoException).code !== 'ENOENT') throw result.error;
  if (process.env.SECBASE_OPTIONAL === '1') {
    // In GitHub Actions the notice becomes a warning annotation on the run's summary page.
    const prefix = process.env.GITHUB_ACTIONS === 'true' ? '::warning::' : '';
    console.warn(
      `${prefix}secbase skipped: not installed; run \`npm run security\` on a machine with secbase`,
    );
    process.exit(0);
  }
  console.error('secbase is not installed: run "npm link" in ~/Documents/secbase');
  process.exit(1);
}
process.exit(result.status ?? 1);
