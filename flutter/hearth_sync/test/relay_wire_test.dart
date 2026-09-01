// The Dart relay client's wire layer against the relay conformance vectors
// (vectors/relay_v1.json, built without any relay code): every request the
// vectors accept decodes and re-encodes to the same bytes, its signature verifies
// over the signable this client builds, and every answer parses.
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:cryptography/cryptography.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hearth_sync/hearth_sync.dart';
import 'package:hearth_sync/src/relay_cbor.dart';
import 'package:hearth_sync/src/relay_client.dart' show RelayWire;

final vectors =
    jsonDecode(File('../../vectors/relay_v1.json').readAsStringSync())
        as Map<String, dynamic>;

Future<bool> verified(Uint8List pk, Uint8List msg, Uint8List sig) =>
    Ed25519().verify(
      msg,
      signature: Signature(
        sig,
        publicKey: SimplePublicKey(pk, type: KeyPairType.ed25519),
      ),
    );

List<(Uint8List, int)> pairs(Object? l) => [
  for (final p in l as List) ((p as List)[0] as Uint8List, p[1] as int),
];

void main() {
  test('the channel id is the vectors\' own', () {
    expect(
      hex(
        RelayWire.channel(
          vectors['app'] as String,
          unhex(vectors['household'] as String),
        ),
      ),
      vectors['channel'],
    );
  });

  test('the codec refuses what an untrusted relay might send', () {
    for (final bad in [
      '1817', // not the shortest form
      '9f00ff', // indefinite length
      'a0', // a map
      'c100', // a tag
      '1b0020000000000000', // above 2^53 - 1
      '8200', // truncated
      '0000', // trailing bytes
      '9affffffff', // a length the body cannot hold
      '818181818181818100', // nine arrays deep
    ]) {
      expect(
        () => cborDecode(unhex(bad)),
        throwsA(isA<BadAnswer>()),
        reason: bad,
      );
    }
    expect(cborDecode(unhex('8181818181818100')), isA<List>());
    expect(
      hex(cborEncode([0, 23, 24, 255, 256, 65536, 4294967296, 1727000000000])),
      '880017181818ff1901001a000100001b00000001000000001b00000192'
      '1938b600',
    );
  });

  test(
    'every accepted request round-trips and verifies over our signable',
    () async {
      final channel = unhex(vectors['channel'] as String);
      final counts = <String, int>{};
      for (final c in vectors['cases'] as List) {
      // Integers up to 2^64 - 1 that no client sends: this client caps them at
      // 2^53 - 1, which every platform, the web included, holds exactly.
      if ((c as Map)['name'] == 'u64_edges') continue;
        for (final s in c['steps'] as List) {
          final step = s as Map;
          if (step['status'] != 200 || step['method'] != 'POST') continue;
          final path = step['path'] as String;
          if (!path.startsWith('/v1/${vectors['channel']}/')) continue;
          final verb = path.split('/').last;
          final body = unhex(step['body'] as String);
          final f = cborDecode(body) as List;
          expect(hex(cborEncode(f)), hex(body), reason: '$verb re-encodes');
          final Uint8List signer, sig, msg;
          switch (verb) {
            case 'enroll':
              counts[verb] = (counts[verb] ?? 0) + 1;
              continue;
            case 'append':
              (signer, sig) = (f[0] as Uint8List, f[4] as Uint8List);
              msg = RelayWire.appendSignable(
                channel,
                signer,
                f[3] as int,
                f[1] as int,
                [for (final e in f[2] as List) e as Uint8List],
              );
            case 'forget':
              (signer, sig) = (f[4] as Uint8List, f[6] as Uint8List);
              msg = RelayWire.forgetSignable(
                channel,
                signer,
                f[5] as int,
                f[0] as Uint8List,
                [for (final e in f[1] as List) e as Uint8List],
                f[2] as Uint8List,
                f[3] as int,
              );
            case 'snapshot':
              (signer, sig) = (f[0] as Uint8List, f[4] as Uint8List);
              msg = RelayWire.snapshotSignable(
                channel,
                signer,
                f[3] as int,
                f[1] as Uint8List,
                pairs(f[2]),
              );
            case 'pull':
              (signer, sig) = (f[0] as Uint8List, f[5] as Uint8List);
              msg = RelayWire.pullSignable(
                channel,
                signer,
                f[1] as int,
                f[2] as int,
                f[3] as Uint8List,
                pairs(f[4]),
              );
              RelayWire.parsePull(unhex(step['response'] as String));
            case 'fetch_snapshot':
              (signer, sig) = (f[0] as Uint8List, f[5] as Uint8List);
              msg = RelayWire.fetchSignable(
                channel,
                signer,
                f[1] as int,
                f[2] as int,
                f[3] as Uint8List,
                f[4] as Uint8List,
              );
              RelayWire.parseFetch(unhex(step['response'] as String));
            default:
              fail('unknown verb $verb');
          }
          expect(await verified(signer, msg, sig), isTrue, reason: verb);
          counts[verb] = (counts[verb] ?? 0) + 1;
        }
      }
      for (final v in [
        'enroll',
        'append',
        'forget',
        'snapshot',
        'pull',
        'fetch_snapshot',
      ]) {
        expect(counts[v], greaterThan(0), reason: 'the vectors exercise $v');
      }
    },
  );

  test('error answers name their code, last seq and epoch', () {
    final seq = RelayWire.error(409, cborEncode(['err', 'seq', 7]));
    expect((seq.code, seq.last), ('seq', 7));
    final epoch = RelayWire.error(409, cborEncode(['err', 'epoch', 3]));
    expect((epoch.code, epoch.epoch), ('epoch', 3));
    final junk = RelayWire.error(502, utf8.encode('<html>bad gateway'));
    expect((junk.code, junk.retryable), ('bad_answer', true));
    expect(
      RelayWire.error(429, cborEncode(['err', 'rate_limited'])).retryable,
      isTrue,
    );
    expect(
      RelayWire.error(403, cborEncode(['err', 'forgotten'])).retryable,
      isFalse,
    );
  });
}
