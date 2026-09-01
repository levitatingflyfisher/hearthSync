// WebCrypto Ed25519 (Chrome 137, Firefox 129, Safari 17). Not run in any test on
// this host: flutter test runs on the VM (kernel v1 stage 3b report).
import 'dart:js_interop';
import 'dart:typed_data';

import 'package:web/web.dart' as web;

/// Whether this page has WebCrypto (a secure context).
bool get webCryptoPresent => web.window.isSecureContext;

/// Import a PKCS#8 Ed25519 key as a non-extractable signing key; null when the
/// browser has no Ed25519 (the import rejects with NotSupportedError).
Future<Object?> webCryptoImport(Uint8List pkcs8) async {
  try {
    final key = await web.window.crypto.subtle
        .importKey(
          'pkcs8',
          pkcs8.toJS,
          'Ed25519'.toJS,
          false,
          <JSString>['sign'.toJS].toJS,
        )
        .toDart;
    return key;
  } catch (_) {
    return null;
  }
}

/// Sign with a key from [webCryptoImport].
Future<Uint8List> webCryptoSign(Object key, Uint8List message) async {
  final sig = await web.window.crypto.subtle
      .sign('Ed25519'.toJS, key as web.CryptoKey, message.toJS)
      .toDart;
  return (sig as JSArrayBuffer).toDart.asUint8List();
}
