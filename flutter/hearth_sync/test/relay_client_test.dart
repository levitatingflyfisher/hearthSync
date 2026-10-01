// The relay client and the loop's scheduling, over package:http's MockClient.
// The real relays are in relay_e2e_test.dart.
import 'dart:math';
import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:hearth_sync/hearth_sync.dart';
import 'package:hearth_sync/src/relay_cbor.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';

import 'support.dart';

http.Response cbor(int status, Object? v) =>
    http.Response.bytes(cborEncode(v), status);

void main() {
  setUpAll(initBridge);

  test('backoff is full jitter under an exponential ceiling', () {
    final r = Random(7);
    for (var n = 1; n <= 12; n++) {
      final ceiling = min(30 * 60 * 1000, 30000 * (1 << (n - 1)));
      final seen = [
        for (var i = 0; i < 200; i++) backoffDelay(n, r).inMilliseconds,
      ];
      expect(seen.every((ms) => ms >= 1000 && ms <= ceiling), isTrue);
      // Jitter: the delays spread over the range rather than bunching at it.
      expect(seen.toSet().length, greaterThan(100), reason: 'n=$n');
    }
  });

  test(
    'a read that names an old epoch learns the new one and signs again',
    () async {
      final epochs = <int>[];
      final relay = RelayClient(
        Uri.parse('https://relay.test/'),
        client: MockClient((req) async {
          expect(req.method, 'POST');
          expect(req.url.path, endsWith('/pull'));
          final body = cborDecode(req.bodyBytes) as List;
          epochs.add(body[2] as int);
          return body[2] == 5
              ? cbor(200, ['ok', 1, [], [], false])
              : cbor(409, ['err', 'epoch', 5]);
        }),
      );
      final signer = SoftwareSigner.fromSeed(List.filled(32, 1));
      final a = await relay.pull(Uint8List(32), signer, const []);
      expect(a.generation, 1);
      expect(epochs, [0, 5]);
      expect((relay.epoch, relay.epochRetries), (5, 1));
      // A second refusal in a row is the caller's to handle.
      final stuck = RelayClient(
        Uri.parse('https://relay.test/'),
        client: MockClient((_) async => cbor(409, ['err', 'epoch', 9])),
      );
      await expectLater(
        stuck.pull(Uint8List(32), signer, const []),
        throwsA(isA<RelayException>().having((e) => e.code, 'code', 'epoch')),
      );
    },
  );

  test(
    'no answer and junk answers are relay errors, retryable or not',
    () async {
      final signer = SoftwareSigner.fromSeed(List.filled(32, 1));
      final down = RelayClient(
        Uri.parse('https://relay.test/'),
        client: MockClient((_) async => throw http.ClientException('refused')),
      );
      await expectLater(
        down.append(Uint8List(32), signer, 1, [Uint8List(1)]),
        throwsA(
          isA<RelayException>()
              .having((e) => e.code, 'code', 'network')
              .having((e) => e.retryable, 'retryable', isTrue),
        ),
      );
      final junk = RelayClient(
        Uri.parse('https://relay.test/'),
        client: MockClient((_) async => cbor(200, ['ok', 'not a seq'])),
      );
      await expectLater(
        junk.append(Uint8List(32), signer, 1, [Uint8List(1)]),
        throwsA(
          isA<RelayException>().having((e) => e.code, 'code', 'bad_answer'),
        ),
      );
    },
  );

  test(
    'a rate-limited round backs off with jitter, and a later round recovers',
    () async {
      var limited = true;
      var requests = 0;
      final clock = TestClock()..now = DateTime.now().millisecondsSinceEpoch;
      final d = await device(1, clock);
      final relay = RelayClient(
        Uri.parse('https://relay.test/'),
        client: MockClient((req) async {
          requests++;
          if (limited) return cbor(429, ['err', 'rate_limited']);
          return switch (req.url.pathSegments.last) {
            'enroll' => cbor(200, ['ok', 1]),
            'pull' => cbor(200, ['ok', 1, [], [], false]),
            'append' => cbor(200, ['ok', 1]),
            _ => cbor(404, ['err', 'not_found']),
          };
        }),
      );
      final loop = SyncLoop(d.hs, relay, random: Random(1));
      final first = await loop.syncNow();
      expect(first.ok, isFalse);
      expect((first.error as RelayException).code, 'rate_limited');
      expect(first.retryIn, isNotNull);
      expect(first.retryIn!.inMilliseconds, inInclusiveRange(1000, 30000));
      final second = await loop.syncNow();
      expect(second.retryIn!.inMilliseconds, inInclusiveRange(1000, 60000));
      expect(requests, 2, reason: 'each round stopped at the refusal');
      limited = false;
      final third = await loop.syncNow();
      expect(third.ok, isTrue, reason: '${third.error}');
      expect(third.result!.enrolled, isTrue);
      expect(third.result!.uploaded, 1, reason: "the device's enrolment");
      expect(await d.hs.relayOutbox(), isEmpty);
      await loop.stop();
    },
  );

  test(
    'rounds never overlap: a request during a round runs once after it',
    () async {
      final clock = TestClock()..now = DateTime.now().millisecondsSinceEpoch;
      final d = await device(1, clock);
      var inFlight = 0, maxInFlight = 0, pulls = 0;
      final relay = RelayClient(
        Uri.parse('https://relay.test/'),
        client: MockClient((req) async {
          inFlight++;
          maxInFlight = max(maxInFlight, inFlight);
          await Future<void>.delayed(const Duration(milliseconds: 5));
          inFlight--;
          return switch (req.url.pathSegments.last) {
            'enroll' => cbor(200, ['ok', 1]),
            'pull' => (() {
              pulls++;
              return cbor(200, ['ok', 1, [], [], false]);
            })(),
            _ => cbor(200, ['ok', 1]),
          };
        }),
      );
      final loop = SyncLoop(d.hs, relay);
      final rounds = [loop.syncNow(), loop.syncNow(), loop.syncNow()];
      await Future.wait(rounds);
      expect(maxInFlight, 1);
      expect(pulls, 2, reason: 'the first round, then one for both requests');
      await loop.stop();
    },
  );

  test(
    'an upload-only round answered seq pulls once, then uploads again',
    () async {
      final clock = TestClock()..now = DateTime.now().millisecondsSinceEpoch;
      final d = await device(1, clock);
      final me = await d.signer.publicKey();
      // A one-device relay: d's log, and how often each verb was asked.
      final log = <Uint8List>[];
      final asked = <String, int>{};
      final relay = RelayClient(
        Uri.parse('https://relay.test/'),
        client: MockClient((req) async {
          final verb = req.url.pathSegments.last;
          asked[verb] = (asked[verb] ?? 0) + 1;
          final body = cborDecode(req.bodyBytes) as List;
          switch (verb) {
            case 'enroll':
              return cbor(200, ['ok', 1]);
            case 'pull':
              final cursors = body[4] as List;
              final from = cursors.isEmpty
                  ? 0
                  : (cursors.first as List)[1] as int;
              return cbor(200, [
                'ok',
                1,
                [
                  if (log.isNotEmpty)
                    [
                      me,
                      1,
                      [
                        for (var i = from; i < log.length; i++) [i + 1, log[i]],
                      ],
                    ],
                ],
                [],
                false,
              ]);
            case 'append':
              final first = body[1] as int;
              final envs = (body[2] as List).cast<Uint8List>();
              if (first != log.length + 1) {
                return cbor(409, ['err', 'seq', log.length]);
              }
              log.addAll(envs);
              return cbor(200, ['ok', log.length]);
          }
          return cbor(404, ['err', 'not_found']);
        }),
      );
      final loop = SyncLoop(d.hs, relay);
      expect((await loop.syncNow()).ok, isTrue);
      // An append the relay stored whose answer never came back: the kernel
      // still holds the op, and its seq is behind the relay's log.
      clock.now += 1;
      await d.hs.put('rooms', 'lost', {'name': 'Cellar'});
      log.addAll([for (final o in await d.hs.relayOutbox()) o.sealed]);
      clock.now += 1;
      await d.hs.put('rooms', 'next', {'name': 'Pantry'});
      asked.clear();
      final s = await loop.syncNow(pull: false);
      expect(s.ok, isTrue, reason: '${s.error}');
      expect(asked['pull'], 1, reason: 'one pull, after the seq answer');
      expect(asked['append'], 2);
      expect(s.result!.uploaded, 1, reason: "only the edit the relay lacked");
      expect(await d.hs.relayOutbox(), isEmpty);
      expect(log, hasLength(3));
      await loop.stop();
    },
  );
}
