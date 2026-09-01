// The signers' backends (ADR 0012, "Signing"): every backend the host can reach
// gives RFC 8032's bytes, the kernel accepts them, and the PKCS#8 wrapping the web
// backend imports is RFC 8410's. WebCrypto itself only runs in a browser, which
// this host test cannot reach (kernel v1 stage 3b report).
import 'dart:convert';
import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:hearth_sync/hearth_sync.dart';

import 'support.dart';

/// RFC 8032 §7.1, TEST 1 and TEST 2: (secret, public, message, signature).
const vectors = [
  (
    '9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60',
    'd75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a',
    '',
    'e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b',
  ),
  (
    '4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb',
    '3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c',
    '72',
    '92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00',
  ),
];

void main() {
  setUpAll(initBridge);

  test(
    'every backend on this host signs RFC 8032 vectors byte for byte',
    () async {
      for (final backend in [SigningBackend.rust, SigningBackend.dart]) {
        for (final (sk, pk, msg, sig) in vectors) {
          final s = SoftwareSigner.fromSeed(unhex(sk), backend: backend);
          expect(await s.backend(), backend);
          expect(hex(await s.publicKey()), pk, reason: '$backend public key');
          expect(
            hex(await s.sign(unhex(msg))),
            sig,
            reason: '$backend signature',
          );
        }
      }
    },
  );

  test('the default on a native host is the Rust bridge', () async {
    final s = SoftwareSigner.fromSeed(List.filled(32, 7));
    expect(await s.backend(), SigningBackend.rust);
    await expectLater(
      SoftwareSigner.fromSeed(
        List.filled(32, 7),
        backend: SigningBackend.webCrypto,
      ).backend(),
      throwsUnsupportedError,
    );
  });

  test('the PKCS#8 wrapping is RFC 8410 §10.3', () {
    const example =
        'MC4CAQAwBQYDK2VwBCIEINTuctv5E1hK1bbY8fdp+K06/nwoy/HU++CXqI9EdVhC';
    final der = base64Decode(example);
    expect(ed25519Pkcs8(der.sublist(16)), der);
    expect(() => ed25519Pkcs8(Uint8List(31)), throwsArgumentError);
  });

  test(
    'the kernel accepts what each backend signs, and a destroyed key refuses',
    () async {
      final clock = TestClock();
      final a = await device(1, clock);
      for (final backend in [SigningBackend.rust, SigningBackend.dart]) {
        final n = backend == SigningBackend.rust ? 2 : 3;
        final signer = SoftwareSigner.fromSeed(
          List.filled(32, n),
          backend: backend,
        );
        final hs = await HearthSync.open(
          app: app,
          schema: support_schema,
          persist: MemoryPersist(),
          signer: signer,
          seed: seed,
          label: 'device $n',
          clock: clock.call,
        );
        final sent = [for (final c in hs.opened) ...c.outgoing];
        clock.now += 1;
        sent.addAll(
          (await hs.put('rooms', 'r$n', {'name': 'Room $n'})).outgoing,
        );
        final got = await a.hs.ingest([for (final s in sent) s.sealed]);
        expect(got.rejected, isEmpty, reason: '$backend');
        expect(a.hs.view().rows.any((r) => r.row == 'r$n'), isTrue);
        await signer.destroy();
        await expectLater(
          signer.sign(Uint8List(1)),
          throwsA(isA<SignerDestroyedException>()),
        );
        await hs.close();
      }
    },
  );
}
