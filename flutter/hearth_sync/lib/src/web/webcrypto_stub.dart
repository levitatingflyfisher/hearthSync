// Off the web there is no WebCrypto: the Rust bridge signs natively instead.
import 'dart:typed_data';

/// Whether this platform has WebCrypto at all (never, off the web).
bool get webCryptoPresent => false;

/// Import a PKCS#8 Ed25519 key as a non-extractable signing key; null when the
/// browser has no Ed25519.
Future<Object?> webCryptoImport(Uint8List pkcs8) async => null;

/// Sign with a key from [webCryptoImport].
Future<Uint8List> webCryptoSign(Object key, Uint8List message) =>
    throw UnsupportedError('WebCrypto is only on the web');
