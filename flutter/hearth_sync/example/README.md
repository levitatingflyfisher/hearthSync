# hearth_sync example

The demo page: two devices in one process, each with its own kernel, key and
record store. A names a room, B adds milk, they sync through `syncWith`, and the
page shows `Sync: PASS` or `FAIL`, how long the first open took, which signing
backend the platform chose, and `Vector: PASS` when that backend signs RFC 8032
test 1 byte for byte. Each result is also printed (the browser console on the
web). It proves
the bridge loads and the Dart API works on the target. It is not an app: it
signs with in-memory demo keys and the debug signing config.

```sh
tool/build_web.sh                                            # PWA, no COOP/COEP needed
flutter build apk --release --target-platform android-arm64,android-x64
```

On the web, `?device=A` opens one device as a web app would instead (records
in IndexedDB, key in flutter_secure_storage) and exposes `window.hearthProbe`
(`put`, `setAdd`, `syncRelay(url)`, `view`, `backend`) for a browser test to
drive (`lib/probe/probe_web.dart`).

Run the two builds one at a time. The web build has been run in
headless Chromium from a plain static server (no COOP/COEP); the APK has not
been run on a device yet.
