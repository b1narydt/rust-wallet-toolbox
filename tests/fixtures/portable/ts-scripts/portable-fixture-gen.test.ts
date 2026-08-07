/* eslint-disable @typescript-eslint/triple-slash-reference */
/// <reference path="../../src/types/bsv-sdk-aesgcm.d.ts" />
/* eslint-enable @typescript-eslint/triple-slash-reference */

// Generates BRC-38/BRC-39 interop fixtures for the Rust wallet-toolbox port.
// Run: FIXTURE_OUT=<dir> npx jest test/storage/portable-fixture-gen.test.ts
// The fixtures are committed in rust-wallet-toolbox/tests/fixtures/portable/.

import * as fs from 'fs'
import * as path from 'path'
import { Utils } from '@bsv/sdk'
import { AESGCM } from '@bsv/sdk/primitives/AESGCM'
import { argon2id } from 'hash-wasm'
import { encryptBRC39, exportBRC38Json, parseBRC38Json, verifyTruthy } from '../../src/index.client'
import { createSyncMap } from '../../src/storage/schema/entities/EntityBase'
import { _tu } from '../utils/TestUtilsWalletStorage'

const outDir = process.env.FIXTURE_OUT ?? path.join(__dirname, 'portable-fixtures-out')

// Decomposed form; the Rust tests use the precomposed "Café fixture pw" to
// prove NFC password normalization crosses implementations.
const password = 'Café fixture pw'

const remoteSyncStorageIdentityKey = 'remote-sync-storage-identity-key'
const remoteSyncStorageName = 'remote-sync-storage'

describe('portable fixture generation', () => {
  jest.setTimeout(99999999)

  test('generate BRC-38/BRC-39 fixtures', async () => {
    fs.mkdirSync(outDir, { recursive: true })

    const ctx = await _tu.createSQLiteTestSetup1Wallet({
      databaseName: 'portable_fixture_gen',
      chain: 'test',
      rootKeyHex: '6'.repeat(64)
    })
    try {
      const storage = ctx.activeStorage
      const user = verifyTruthy(ctx.setup?.u1)
      const proven = await _tu.insertTestProvenTx(storage)
      const { tx } = await _tu.insertTestTransaction(storage, user, false, {
        txid: proven.txid,
        provenTxId: proven.provenTxId
      })
      const req = await _tu.insertTestProvenTxReq(storage, proven.txid, proven.provenTxId)
      const remoteSyncState = await _tu.insertTestSyncState(storage, user)
      const remoteSyncMap = createSyncMap()
      remoteSyncMap.transaction.idMap[777] = tx.transactionId
      remoteSyncMap.output.idMap[778] = ctx.setup!.u1tx1o0.outputId
      remoteSyncMap.outputBasket.idMap[779] = ctx.setup!.u1basket1.basketId
      remoteSyncMap.txLabel.idMap[780] = ctx.setup!.u1label1.txLabelId
      remoteSyncMap.outputTag.idMap[781] = ctx.setup!.u1tag1.outputTagId
      remoteSyncMap.certificate.idMap[782] = ctx.setup!.u1cert1.certificateId
      remoteSyncMap.commission.idMap[783] = ctx.setup!.u1comm1.commissionId
      remoteSyncMap.provenTx.idMap[784] = proven.provenTxId
      remoteSyncMap.provenTxReq.idMap[785] = req.provenTxReqId
      await storage.updateSyncState(remoteSyncState.syncStateId, {
        storageIdentityKey: remoteSyncStorageIdentityKey,
        storageName: remoteSyncStorageName,
        syncMap: JSON.stringify(remoteSyncMap)
      })

      // F1: canonical BRC-38 JSON straight from the TS exporter.
      const json = await exportBRC38Json(storage, ctx.identityKey)
      fs.writeFileSync(path.join(outDir, 'brc38-ts-export.json'), json)

      // F2: BRC-39 with default KDF params over the SAME document, so the
      // decrypted plaintext must equal F1 byte-for-byte.
      const brc39 = await encryptBRC39(parseBRC38Json(json), password)
      fs.writeFileSync(path.join(outDir, 'brc39-ts-default.bin'), Buffer.from(brc39))

      // F3: fully deterministic BRC-39 (fixed salt/nonce, cheap KDF) over F1.
      // Lets Rust prove byte-identical ENCRYPTION (argon2id key + AES-GCM
      // framing), not just successful decryption.
      const iterations = 1
      const memoryKiB = 64
      const salt = new Uint8Array(32).fill(7)
      const nonce = new Uint8Array(32).fill(9)
      const key = new Uint8Array(
        await argon2id({
          password: new Uint8Array(Utils.toArray(password.normalize('NFC'), 'utf8')),
          salt,
          iterations,
          memorySize: memoryKiB,
          parallelism: 1,
          hashLength: 32,
          outputType: 'binary'
        })
      )
      const encrypted = AESGCM(new Uint8Array(Utils.toArray(json, 'utf8')), nonce, key)
      const header = new Uint8Array(33 + 64)
      header.set([0x57, 0x44, 0x41, 0x54], 0)
      header[4] = 1
      header[5] = 1
      header[6] = 38
      header[7] = 1
      header[9] = 32
      header[10] = 32
      writeUInt32BE(header, 11, iterations)
      writeUInt32BE(header, 15, memoryKiB)
      header[19] = 1
      header[20] = 32
      header.set(salt, 33)
      header.set(nonce, 65)
      const file = Buffer.concat([
        Buffer.from(header),
        Buffer.from(encrypted.result),
        Buffer.from(encrypted.authenticationTag)
      ])
      fs.writeFileSync(path.join(outDir, 'brc39-ts-lowcost.bin'), file)

      fs.writeFileSync(
        path.join(outDir, 'meta.json'),
        JSON.stringify(
          {
            identityKey: ctx.identityKey,
            passwordNfd: password,
            passwordNfc: password.normalize('NFC'),
            lowcost: { iterations, memoryKiB, parallelism: 1, saltByte: 7, nonceByte: 9 }
          },
          null,
          2
        )
      )
      expect(json.startsWith('{"brc":38,')).toBe(true)
    } finally {
      await ctx.storage.destroy()
    }
  })
})

function writeUInt32BE (target: Uint8Array, offset: number, value: number): void {
  target[offset] = (value >>> 24) & 0xff
  target[offset + 1] = (value >>> 16) & 0xff
  target[offset + 2] = (value >>> 8) & 0xff
  target[offset + 3] = value & 0xff
}
