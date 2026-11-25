import { expect, it } from 'vitest'
import QRCode from 'qrcode'
import jsQR from 'jsqr'

it.each(['', 'SONIMESH-LAB1:65535:'])('encodes a full-size lab payload with prefix %s', (prefix) => {
  const invitation = prefix + 'aBcD'.repeat(417)
  const text = invitation.toUpperCase()
  const qr = QRCode.create(text, { errorCorrectionLevel: 'M' })
  const scale = 4
  const margin = 4
  const width = (qr.modules.size + margin * 2) * scale
  const pixels = new Uint8ClampedArray(width * width * 4).fill(255)
  for (let row = 0; row < qr.modules.size; row++) {
    for (let column = 0; column < qr.modules.size; column++) {
      if (!qr.modules.get(row, column)) continue
      for (let y = 0; y < scale; y++) for (let x = 0; x < scale; x++) {
        const offset = (((row + margin) * scale + y) * width + (column + margin) * scale + x) * 4
        pixels[offset] = pixels[offset + 1] = pixels[offset + 2] = 0
      }
    }
  }
  expect(jsQR(pixels, width, width)?.data).toBe(text)
})
