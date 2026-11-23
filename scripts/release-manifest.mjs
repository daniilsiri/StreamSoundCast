import { readFile, writeFile } from 'node:fs/promises'
import { basename, dirname, join } from 'node:path'

const installer = process.argv[2]
if (!installer) throw new Error('Pass the signed Windows x64 NSIS installer path.')

const { productName, version } = JSON.parse(await readFile('src-tauri/tauri.conf.json', 'utf8'))
const tag = `v${version}`
const name = basename(installer)
if (!/^\d+\.\d+\.\d+$/.test(version) || name !== `${productName}_${version}_x64-setup.exe`) {
  throw new Error('The installer name must match the product and release version.')
}
const signature = (await readFile(`${installer}.sig`, 'utf8')).trim()
if (!signature) throw new Error('The installer signature is empty.')

const manifest = {
  version,
  pub_date: new Date().toISOString(),
  platforms: {
    'windows-x86_64': {
      signature,
      url: `https://github.com/daniilsiri/StreamSoundCast/releases/download/${tag}/${encodeURIComponent(name)}`,
    },
  },
}

await writeFile(join(dirname(installer), 'latest.json'), JSON.stringify(manifest, null, 2) + '\n')
