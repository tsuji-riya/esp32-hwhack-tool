// Compress every file in dist/ so the firmware can embed and serve them with
// `Content-Encoding: gzip`. build.rs picks up `*.gz` and ignores the plain originals.
import {readdirSync, readFileSync, rmSync, writeFileSync} from 'node:fs'
import {join} from 'node:path'
import {gzipSync} from 'node:zlib'

const dist = new URL('../dist', import.meta.url).pathname
let total = 0
for (const name of readdirSync(dist)) {
  if (name.endsWith('.gz')) continue
  const path = join(dist, name)
  const gz = gzipSync(readFileSync(path), {level: 9})
  writeFileSync(`${path}.gz`, gz)
  rmSync(path)
  total += gz.length
  console.log(`${name.padEnd(12)} ${(gz.length / 1024).toFixed(1)} KiB gzip`)
}
console.log(`total embedded: ${(total / 1024).toFixed(1)} KiB`)
