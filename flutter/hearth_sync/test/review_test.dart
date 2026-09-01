import 'package:flutter_test/flutter_test.dart';
import 'package:hearth_sync/hearth_sync.dart';

import 'support.dart';

void main() {
  setUpAll(initBridge);

  test(
    'a device back past the horizon rebases through several signatures and keeps its review list',
    () async {
      const h = 20000; // the test schema's horizon, in millis
      final clock = TestClock();
      final a = await device(1, clock);
      final b = await device(2, clock);
      clock.now += 1;
      await a.hs.put('items', 'i1', {'name': 'Lamp'});
      await a.hs.put('items', 'i2', {'name': 'Rug'});
      clock.now += 10;
      await a.hs.syncWith(b.hs);
      // B goes away and edits: one field nobody else touches, one A changes too, and
      // a set add.
      clock.now += 1;
      await b.hs.put('items', 'i1', {'name': 'Lamp (B)'});
      await b.hs.put('items', 'i2', {'name': 'Rug (B)'});
      await b.hs.setAdd('tags', 'new');
      await a.hs.put('items', 'i2', {'name': 'Rug (A)'});
      await a.hs.checkpoint();
      // A's checkpoint passes the horizon before B comes back.
      clock.now += h + 100;
      b.signer.signed = 0;
      b.log.clear();
      await b.hs.syncWith(a.hs);
      expect(
        b.signer.signed,
        greaterThanOrEqualTo(2),
        reason: 'one signature per re-issued op',
      );
      final adopted = b.log.firstWhere((c) => c.reissued > 0);
      expect(adopted.reissued, 2, reason: "i1's edit and the set add");
      expect(adopted.reviewAdded, hasLength(1));
      final item = adopted.reviewAdded.single;
      expect(
        (item.kind, item.table, item.row, item.field),
        ('field', 'items', 'i2', 'name'),
      );
      expect((item.mine, item.current), ('Rug (B)', 'Rug (A)'));
      expect(b.mirror.rows['items/i1']?['name'], 'Lamp (B)');
      expect(b.mirror.rows['items/i2']?['name'], 'Rug (A)');
      b.check();
      clock.now += 1;
      await a.hs.syncWith(b.hs);
      expect(a.mirror.snapshot, b.mirror.snapshot);

      // The review list survives a restart (the records were reset by the snapshot
      // and rewritten), and dismissing an entry deletes it for good.
      await b.hs.close();
      clock.now += 1;
      final again = await Device(2, b.persist, clock).open();
      expect(again.hs.review().map((e) => hex(e.key)), [hex(item.key)]);
      expect(again.hs.review().single.mine, 'Rug (B)');
      // The snapshot replaced B's log: the ops it rebased away must not come back
      // from stale records and reach A.
      a.log.clear();
      clock.now += 1;
      await again.hs.syncWith(a.hs);
      expect(
        [for (final c in a.log) ...c.delivered],
        isEmpty,
        reason: 'B had nothing A lacks after the restart',
      );
      expect(Mirror.of(again.hs).snapshot, a.mirror.snapshot);
      await again.hs.dismissReview(item.key);
      expect(again.hs.review(), isEmpty);
      await again.hs.close();
      final third = await Device(2, b.persist, clock).open();
      expect(third.hs.review(), isEmpty);
    },
  );
}
