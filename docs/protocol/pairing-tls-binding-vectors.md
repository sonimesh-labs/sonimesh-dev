# Pairing TLS-Binding Test Vector

This vector fixes the inputs and byte layout that transform a successful pinned
TLS 1.3 handshake into role-separated pairing proofs and the 32-byte media root
consumed by ADR 0005. It contains intentionally public test values only. It is
not a QR payload, certificate, production pairing secret, live TLS transcript,
or evidence that an Android or Windows TLS provider supports exporters.

## TLS exporter requirement

After completing TLS 1.3 and validating the desktop SPKI against the QR pin,
both peers call their TLS provider's exporter API exactly once for the pairing
binding:

| Field | Value |
| --- | --- |
| Exporter label ASCII | `EXPORTER-SoniMesh Pairing v1` |
| Exporter context | Exact 16-byte `pairing_id` from the QR payload |
| Context flag | Enabled / supplied |
| Requested output | 32 bytes |

The returned 32 bytes are `tls_exporter`. They must not be logged, placed in a
control frame, stored in a trust record, or reused by a later TLS connection. A
provider error, non-TLS-1.3 handshake, pin failure, absent context support, or
output of any other length fails pairing closed.

## Public vector inputs

| Field | Hex value |
| --- | --- |
| Protocol version | `01` |
| Pairing ID | `000102030405060708090a0b0c0d0e0f` |
| Pairing secret | `101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f` |
| TLS exporter | `a0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebf` |

The vector supplies `tls_exporter` directly to the pure HKDF function. A live
provider spike must separately obtain the same length output from its own TLS
handshake; no implementation may derive it from these public bytes.

## Fixed derivation

All byte strings below are concatenated with no length prefix. Integer fields
are their stated fixed-width byte representation.

~~~text
prk = HKDF-Extract(
  salt = pairing_secret,
  ikm = tls_exporter,
  hash = SHA-256,
)

client_proof = HKDF-Expand(
  prk,
  "sonimesh pairing proof v1" || protocol_version_u8 || 0x00 || pairing_id,
  32,
)

server_proof = HKDF-Expand(
  prk,
  "sonimesh pairing proof v1" || protocol_version_u8 || 0x01 || pairing_id,
  32,
)

media_root = HKDF-Expand(
  prk,
  "sonimesh pairing media root v1" || protocol_version_u8 || pairing_id,
  32,
)
~~~

`0x00` means Android/client and `0x01` means desktop/server. Each peer sends
only its own 32-byte proof inside the pinned TLS channel and verifies the
opposite role's expected proof in constant time. Pairing helpers take the
endpoint's local role and derive the opposite expected role internally; a
pairing owner must not select an arbitrary expected peer role at the verification
call site. Reflection of a client proof as a server proof must fail.
`media_root` must remain in zeroizing secret storage and becomes the existing
`SessionSecret` input for ADR 0005; it is not transmitted in the control
protocol.

## Expected outputs

| Field | Expected hex |
| --- | --- |
| Client proof | `c274dfe72135ae558034ca1e8a27a4eab7aac9b87a0cd93c17bf17126e27f4f2` |
| Server proof | `f347ab32006e8acb220f91b0e826bbd58257c55773d7284f8794217f896b4601` |
| Media root | `25b4e89fb655ed5a9a9ad9b28e6ac7e6d49c1246757b75f20a120167bfd2452a` |

Implementations must reject a changed pairing ID, pairing secret, exporter,
protocol version, or peer role. They must not accept a proof from another TLS
connection, another QR invitation, or the same role. These pure tests do not
consume a pairing record, create a TLS socket, trust a certificate, persist an
identity, or enable network pairing.
