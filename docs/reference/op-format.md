# Op format v1 (reference)

Everything is deterministic CBOR (RFC 8949 §4.2.1 plus the dCBOR profile:
shortest integer heads, definite lengths, map keys sorted by encoded bytes, NFC
text, no floats). `vectors/make_vectors.py` is the executable twin of this page
and `vectors/ops_v1.json` its output; every implementation must reproduce those
bytes.

## The op

```
{ 0: 1,                    ; format version
  1: app,                  ; text, ^[a-z0-9]+$, not a reserved word
  2: device,               ; bstr 32, Ed25519 public key
  3: [parent, ...],        ; bstr 32 each, sorted ascending, unique, <= 64
  4: [millis, counter],    ; uint, uint (counter < 2^32)
  5: body,                 ; see below
  6: sig }                 ; bstr 64, Ed25519 over the same map without key 6
```

`id = SHA-256(encoded signed map)`. At most 64 KiB encoded.

## Bodies

| Kind | Array |
|---|---|
| 0 Put | `[0, table, row, {field: value, ...}]` (at least one field), or `[0, table, row, {...}, [millis, counter]]` for a re-issued edit: the 5th item is the original edit's clock (`origin`), strictly older than the op's own clock (else `schema`). LWW orders a field by `(origin or own clock, own clock, id)` (ADR 0006, OriginClock) |
| 1 Delete | `[1, table, row, [observed put ids]]` |
| 2 Restore | `[2, table, row, [delete ids]]` |
| 3 SetAdd | `[3, set, element]` |
| 4 SetRemove | `[4, set, element, [observed add ids]]` |
| 5 Append | `[5, stream, record]` |
| 6 Enroll | `[6, device, label, auth]` |
| 7 Forget | `[7, device, [cut ids], auth]` |
| 8 Checkpoint | `[8, state_hash]`: SHA-256 of dCBOR `[base state bytes, [index entries of before(C) in clock order], [kept op ids, sorted]]`. The base state is the fold of the ops some checkpoint in upto(C) by another device backs; the kept ids are the rest of before(C), which a snapshot carries as bodies (ADRs 0005, 0007) |

Names (table, row, field, set, stream, label) are text of 1–128 bytes. A value
is `null`, `true`/`false`, an integer in i64 range, text, or bytes. Id lists are
sorted ascending with no duplicates.

## Enroll-key authorisations

The enroll key is `HKDF-SHA256(ikm = 64-byte seed, salt = none, info =
"openhearth.<app>.enroll.v1")`, 32 bytes, used as an Ed25519 secret key.

- Enroll `auth` signs `["oh-enroll/v1", app, device, label]`.
- Forget `auth` signs `["oh-forget/v1", app, device, [cut ids]]`.

## Rejection codes

`too_large`, `not_canonical`, `schema`, `wrong_app`, `bad_signature`,
`clock_not_after_parents`, `not_enrolled`, `bad_enroll_auth`,
`observed_not_in_past`, `parent_rejected`, `schema_violation` (V7, ADR 0009).
See ADR 0004 for which rule each enforces.
