import { spawnSync } from 'node:child_process';
import { readdir, readFile } from 'node:fs/promises';

const url = 'http://forms.test/workload';
const output = '/evidence/onefile';
const result = spawnSync('node', ['/src/onefile-cli/src/cli.ts', 'save', url, '--backend', 'helium',
  '--cdp-url', process.env.CDP_ENDPOINT || 'http://127.0.0.1:9221', '--out', output], { encoding: 'utf8', timeout: 90000 });
const files = await readdir(output).catch(() => []);
const archives = await Promise.all(files.map(async name => ({ name, bytes: (await readFile(`${output}/${name}`)).length })));
console.log(JSON.stringify({ exit: result.status, stdout: result.stdout, stderr: result.stderr, archives,
  saved: result.status === 0 && archives.some(file => file.bytes > 0) }));
