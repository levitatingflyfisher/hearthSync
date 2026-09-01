import 'dart:typed_data';

import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:hearth_sync/hearth_sync.dart';

import 'support.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUpAll(initBridge);

  test(
    'a forgotten device learns it on sync, destroys its key and keeps its records',
    () async {
      final clock = TestClock();
      final a = await device(1, clock);
      final b = await device(2, clock);
      clock.now += 1;
      await a.hs.syncWith(b.hs);
      final bid = await b.signer.publicKey();
      await a.hs.forgetDevice(bid);
      expect(a.hs.devices().where((d) => d.forgotten).single.device, bid);
      clock.now += 1;
      await b.hs.syncWith(a.hs);
      expect(
        b.log.any((c) => c.wiped),
        isTrue,
        reason: 'the changes tell the app to delete the words',
      );
      expect(b.hs.isWiped, isTrue);
      expect(b.signer.destroyed, isTrue, reason: 'the device key is gone');
      await expectLater(
        b.signer.inner.sign(Uint8List(1)),
        throwsA(isA<SignerDestroyedException>()),
      );
      await expectLater(
        b.hs.setAdd('tags', 'x'),
        throwsA(
          isA<HearthSyncException>().having((e) => e.code, 'code', 'no_keys'),
        ),
      );
      // Without keys, the records still say whose they were and that it was wiped;
      // opening them is refused.
      final info = await HearthSync.storedDevice(b.persist);
      expect(sameBytes(info!.device, bid), isTrue);
      expect(info.wiped, isTrue);
      await expectLater(
        Device(2, b.persist, clock).open(),
        throwsA(isA<DeviceWipedException>()),
      );
    },
  );

  test(
    'a device that forgets itself hands its ops and Forget over after a restart, without keys',
    () async {
      final clock = TestClock();
      final a = await device(1, clock);
      final c = await device(3, clock);
      clock.now += 1;
      await a.hs.syncWith(c.hs);
      // C writes offline, then forgets itself before any of it reaches A.
      await c.hs.put('rooms', 'attic', {'name': 'Attic'});
      final wiped = await c.hs.forgetSelf();
      expect(wiped.wiped, isTrue);
      expect(c.signer.destroyed, isTrue);
      await c.hs.close();

      // The app restarts. The words are gone; the records are not. A key still
      // in the store (the app stopped mid-handover) is destroyed by open.
      clock.now += 1;
      final again = Device(3, c.persist, clock);
      await expectLater(again.open(), throwsA(isA<DeviceWipedException>()));
      expect(again.signer.destroyed, isTrue);
      final out = await HearthSync.wipedHandover(
        persist: c.persist,
        schema: schema(),
        clock: clock.call,
      );
      // Byte for byte what C first sent the relay (the envelope is deterministic).
      final first = {for (final s in c.sent) hex(s.id): hex(s.sealed)};
      for (final s in c.sent) {
        expect(out.map((o) => hex(o.id)), contains(hex(s.id)));
      }
      for (final o in out.where((o) => first.containsKey(hex(o.id)))) {
        expect(hex(o.sealed), first[hex(o.id)]);
      }
      final got = await a.hs.ingest([for (final o in out) o.sealed]);
      expect(got.rejected, isEmpty);
      expect(a.hs.status().pending, 0);
      expect(
        a.hs.devices().where((d) => d.label == 'device 3').single.forgotten,
        isTrue,
        reason: 'A learns C forgot itself',
      );
      expect(a.mirror.rows['rooms/attic'], {
        'name': 'Attic',
      }, reason: "C's offline edit arrives");
      // Once handed on, the records can go.
      await c.persist.clear();
      expect(await HearthSync.storedDevice(c.persist), isNull);
    },
  );

  test(
    'the secure-storage key survives a restart and is deleted on wipe',
    () async {
      FlutterSecureStorage.setMockInitialValues({});
      final first = SecureStorageSigner();
      final pk = await first.publicKey();
      expect(pk.length, 32);
      final again = SecureStorageSigner();
      expect(
        await again.publicKey(),
        pk,
        reason: 'the same key after a restart',
      );
      // The kernel checks the signature under the device key: open, enrol, write.
      final clock = TestClock();
      final hs = await HearthSync.open(
        app: app,
        schema: schema(),
        persist: MemoryPersist(),
        signer: again,
        seed: seed,
        clock: clock.call,
      );
      await hs.put('rooms', 'den', {'name': 'Den'});
      expect(hs.view().rows.single.fields, {'name': 'Den'});
      await again.destroy();
      await expectLater(
        again.sign(Uint8List(1)),
        throwsA(isA<SignerDestroyedException>()),
      );
      final fresh = SecureStorageSigner();
      expect(
        sameBytes(await fresh.publicKey(), pk),
        isFalse,
        reason: 'the old key is gone',
      );
    },
  );
}
