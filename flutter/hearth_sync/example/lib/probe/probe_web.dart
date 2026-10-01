// The browser probe: `?device=A` opens one device the way a web app would (records
// in IndexedDB, key in flutter_secure_storage) and exposes `window.hearthProbe`
// for a browser test to drive: put, setAdd, syncRelay(url), view, backend. Every
// function returns a Promise of a JSON string; failures come back as {"error"}.
import 'dart:convert';
import 'dart:js_interop';
import 'dart:js_interop_unsafe';

import 'package:hearth_sync/hearth_sync.dart';
import 'package:idb_shim/idb_browser.dart';
import 'package:web/web.dart' as web;

import '../shared.dart';

Future<String?> startProbe() async {
  final name = Uri.base.queryParameters['device'];
  if (name == null) return null;
  try {
    final signer = SecureStorageSigner(key: 'hearth_probe.$name');
    final hs = await HearthSync.open(
      app: 'lullaby',
      schema: demoSchema,
      persist: IdbPersist(idbFactoryBrowser, dbName: 'hearth_probe_$name'),
      signer: signer,
      seed: demoSeed,
      label: 'browser $name',
    );
    // SecureStorageSigner wraps a SoftwareSigner; ask one over the same seed
    // path which backend this browser gives (the choice depends only on the
    // platform, not on the seed).
    final backend = await SoftwareSigner.fromSeed(List.filled(32, 7)).backend();
    final clients = <String, RelayClient>{};

    JSPromise<JSString> run(Future<Object?> Function() f) => () async {
      try {
        return jsonEncode(await f()).toJS;
      } catch (e) {
        return jsonEncode({'error': '$e'}).toJS;
      }
    }().toJS;

    Map<String, Object?> view() {
      final v = hs.view();
      return {
        'rows': {for (final r in v.rows) r.row: r.fields['name']},
        'sets': [for (final s in v.sets) s.element]..sort(),
        'devices': [for (final d in hs.devices()) d.label]..sort(),
      };
    }

    final api = JSObject()
      ..setProperty('backend'.toJS, (() => run(() async => backend.name)).toJS)
      ..setProperty('view'.toJS, (() => run(() async => view())).toJS)
      ..setProperty(
        'put'.toJS,
        ((JSString row, JSString n) => run(() async {
          final c = await hs.put('rooms', row.toDart, {'name': n.toDart});
          return {'rows': c.rows.length, 'outgoing': c.outgoing.length};
        })).toJS,
      )
      ..setProperty(
        'setAdd'.toJS,
        ((JSString el) => run(() async {
          final c = await hs.setAdd('groceries', el.toDart);
          return {'sets': c.sets.length, 'outgoing': c.outgoing.length};
        })).toJS,
      )
      ..setProperty(
        'syncRelay'.toJS,
        ((JSString url) => run(() async {
          final relay = clients[url.toDart] ??= RelayClient(
            Uri.parse(url.toDart),
          );
          final r = await hs.syncWithRelay(relay);
          return {
            'enrolled': r.enrolled,
            'pulled': r.pulled,
            'uploaded': r.uploaded,
            'epoch': relay.epoch,
          };
        })).toJS,
      );
    web.window.setProperty('hearthProbe'.toJS, api);
    final status =
        'PROBE READY device=$name backend=${backend.name} '
        'view=${jsonEncode(view())}';
    // ignore: avoid_print
    print(status);
    return status;
  } catch (e) {
    final status = 'PROBE FAILED device=$name: $e';
    // ignore: avoid_print
    print(status);
    return status;
  }
}
