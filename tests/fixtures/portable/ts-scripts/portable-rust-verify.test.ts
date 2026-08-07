// Verifies Rust-produced BRC-38/BRC-39 artifacts against the TS
// implementation -- the Rust -> TS direction of the interop proof.
// Artifacts are produced by rust-wallet-toolbox:
//   RUST_ARTIFACT_OUT=1 cargo test --test portable_tests generate_rust_export_artifacts
// Run: RUST_FIXTURES=<rust-wallet-toolbox>/tests/fixtures/portable \
//        npx jest test/storage/portable-rust-verify.test.ts

import * as fs from 'fs'
import * as path from 'path'
import { decryptBRC39, exportBRC38, importBRC38, parseBRC38Json, verifyTruthy } from '../../src/index.client'
import { StorageKnex } from '../../src/storage/StorageKnex'
import { _tu } from '../utils/TestUtilsWalletStorage'

const fixtureDir = process.env.RUST_FIXTURES ?? path.join(__dirname, 'portable-fixtures-out')
// Precomposed form of the NFD password the fixtures were generated with.
const password = 'Café fixture pw'

describe('Rust-produced portable artifacts', () => {
  jest.setTimeout(99999999)

  const maybe = fs.existsSync(path.join(fixtureDir, 'brc39-rust-export.bin')) ? test : test.skip

  maybe('BRC-39 file encrypted by Rust decrypts and restores in TS', async () => {
    const bytes = Array.from(fs.readFileSync(path.join(fixtureDir, 'brc39-rust-export.bin')))
    const decrypted = await decryptBRC39(bytes, password)

    // The decrypted document must equal the Rust BRC-38 export byte-wise.
    const rustJson = fs.readFileSync(path.join(fixtureDir, 'brc38-rust-export.json'), 'utf8')
    expect(decrypted).toEqual(parseBRC38Json(rustJson))

    // And it must restore into an empty TS storage, then re-export with
    // identical user and tables sections.
    const localSQLiteFile = await _tu.newTmpFile('rust_verify_target.sqlite', false, false, false)
    const storage = new StorageKnex({
      ...StorageKnex.defaultOptions(),
      chain: 'test',
      knex: _tu.createLocalSQLite(localSQLiteFile)
    })
    try {
      await storage.dropAllData()
      await storage.migrate('rust_verify_target', '9'.repeat(64))
      await storage.makeAvailable()
      const result = await importBRC38(storage, decrypted, { mode: 'restore' })
      expect(result.mode).toBe('restore')
      const identityKey = verifyTruthy(decrypted.user.identityKey as string)
      const reexported = await exportBRC38(storage, identityKey)
      expect(reexported.user).toEqual(decrypted.user)
      // The Rust storage schema does not model provenTxReq.wasBroadcast /
      // rebroadcastAttempts (rust-wallet-toolbox#34), so its exports omit
      // them; the TS re-export re-adds the column defaults. Strip those two
      // fields before comparing -- the only sanctioned normalization.
      for (const req of reexported.tables.provenTxReqs) {
        delete req.wasBroadcast
        delete req.rebroadcastAttempts
      }
      expect(reexported.tables).toEqual(decrypted.tables)
    } finally {
      await storage.destroy()
    }
  })
})
