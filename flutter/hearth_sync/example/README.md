# hearth_sync example

The demo page: two devices in one process, each with its own kernel, key and
record store. A names a room, B adds milk, they sync through `syncWith`, and the
page shows `Sync: PASS` or `FAIL` and how long the first open took. It proves
the bridge loads and the Dart API works on the target. It is not an app: it
signs with in-memory demo keys and the debug signing config.

```sh
tool/build_web.sh                                            # PWA, no COOP/COEP needed
flutter build apk --release --target-platform android-arm64,android-x64
```

Run both through the workshop's `heavy.sh`. Neither has been run in a browser
or on a device yet.
