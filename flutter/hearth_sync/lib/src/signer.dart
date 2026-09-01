import 'dart:convert';
import 'dart:math';
import 'dart:typed_data';

import 'package:cryptography/cryptography.dart';
import 'package:cryptography/dart.dart';
import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter_secure_storage/flutter_secure_storage.dart';

import 'rust/api/signing.dart';
import 'rust/frb_generated.dart';
import 'web/webcrypto_stub.dart'
    if (dart.library.js_interop) 'web/webcrypto_web.dart';

/// The device key, kept outside the kernel (ADR 0011). The kernel hands over the
/// exact bytes to sign; the signer returns an Ed25519 signature over them.
abstract interface class Signer {
  /// The device's Ed25519 public key (32 bytes), creating the key if needed.
  Future<Uint8List> publicKey();

  /// An Ed25519 signature (64 bytes) over [message].
  Future<Uint8List> sign(Uint8List message);

  /// Delete the key for good: the device was forgotten and wiped.
  Future<void> destroy();
}

/// Thrown by a signer whose key was destroyed.
class SignerDestroyedException implements Exception {
  @override
  String toString() => 'SignerDestroyedException: the device key was wiped';
}

/// Where a signer computes its Ed25519 signatures (ADR 0012, "Signing"). All three
/// give the same bytes: Ed25519 (RFC 8032) is deterministic.
enum SigningBackend {
  /// The browser's WebCrypto Ed25519 (Chrome 137, Firefox 129, Safari 17), with
  /// the key imported non-extractable.
  webCrypto,

  /// The Rust bridge (`ed25519_sign`, over the kernel's own signer): native code
  /// on the platforms the plugin builds (Android and Linux today), WASM on the
  /// web. It keeps nothing: the seed
  /// comes with each message.
  rust,

  /// Pure Dart (package:cryptography): only where neither of the others is
  /// available, e.g. before [HearthSync.init] has loaded the bridge.
  dart,
}

/// RFC 8410's PKCS#8 wrapping of a 32-byte Ed25519 seed, the form WebCrypto
/// imports a private key in.
Uint8List ed25519Pkcs8(List<int> seed) {
  if (seed.length != 32) {
    throw ArgumentError.value(seed.length, 'seed', 'must be 32 bytes');
  }
  return Uint8List.fromList([
    // SEQUENCE { INTEGER 0, SEQUENCE { OID 1.3.101.112 }, OCTET STRING { OCTET STRING seed } }
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, //
    0x04, 0x22, 0x04, 0x20, ...seed,
  ]);
}

/// A signer over a 32-byte Ed25519 seed held in memory: the base of
/// [SecureStorageSigner], and for tests. It signs with the fastest
/// [SigningBackend] this platform has (WebCrypto on the web, else the Rust
/// bridge, else pure Dart), or the one passed as [backend].
class SoftwareSigner implements Signer {
  /// A signer from a 32-byte Ed25519 seed.
  SoftwareSigner.fromSeed(List<int> seed, {SigningBackend? backend})
    : _seed = Uint8List.fromList(seed),
      _wanted = backend {
    if (seed.length != 32) {
      throw ArgumentError.value(seed.length, 'seed', 'must be 32 bytes');
    }
  }

  static final _ed = DartEd25519();
  Uint8List? _seed;
  final SigningBackend? _wanted;
  Future<SigningBackend>? _chosen;
  Object? _webKey;
  SimpleKeyPair? _pair;

  static bool get _bridge => RustLib.instance.initialized;

  Uint8List _live() => _seed ?? (throw SignerDestroyedException());

  /// The backend this signer signs with, chosen on first use.
  Future<SigningBackend> backend() => _chosen ??= _choose();

  Future<SigningBackend> _choose() async {
    final seed = _live();
    final wanted = _wanted;
    if (wanted == null || wanted == SigningBackend.webCrypto) {
      if (kIsWeb && webCryptoPresent) {
        _webKey = await webCryptoImport(ed25519Pkcs8(seed));
        if (_webKey != null) return SigningBackend.webCrypto;
      }
      if (wanted != null) {
        throw UnsupportedError('this platform has no WebCrypto Ed25519');
      }
    }
    if (wanted == null || wanted == SigningBackend.rust) {
      if (_bridge) return SigningBackend.rust;
      if (wanted != null) {
        throw StateError('the bridge is not loaded: call HearthSync.init');
      }
    }
    return SigningBackend.dart;
  }

  Future<SimpleKeyPair> _keyPair() async =>
      _pair ??= await _ed.newKeyPairFromSeed(_live());

  @override
  Future<Uint8List> publicKey() async {
    // WebCrypto will not hand out the public half of a non-extractable key.
    if (_bridge && _wanted != SigningBackend.dart) {
      return ed25519PublicKey(seed: _live());
    }
    final pk = await (await _keyPair()).extractPublicKey();
    return Uint8List.fromList(pk.bytes);
  }

  @override
  Future<Uint8List> sign(Uint8List message) async {
    final b = await backend();
    final seed = _live();
    switch (b) {
      case SigningBackend.webCrypto:
        return webCryptoSign(_webKey!, message);
      case SigningBackend.rust:
        return ed25519Sign(seed: seed, message: message);
      case SigningBackend.dart:
        final sig = await _ed.sign(message, keyPair: await _keyPair());
        return Uint8List.fromList(sig.bytes);
    }
  }

  @override
  Future<void> destroy() async {
    _seed?.fillRange(0, 32, 0);
    _seed = null;
    _pair = null;
    _webKey = null;
  }
}

/// The device key as a random Ed25519 seed in the platform's secure storage
/// (Android Keystore-wrapped preferences, the browser's storage on the web):
/// ruling Q6's software key for v1. It signs as [SoftwareSigner] does, with the
/// fastest backend the platform has.
class SecureStorageSigner implements Signer {
  /// A signer whose seed lives under [key] in [storage].
  SecureStorageSigner({
    FlutterSecureStorage storage = const FlutterSecureStorage(),
    this.key = 'hearth_sync.device_key.v1',
    this.backend,
  }) : _storage = storage;

  final FlutterSecureStorage _storage;

  /// Force a [SigningBackend] (null: the fastest available).
  final SigningBackend? backend;

  /// The storage key holding the seed (base64).
  final String key;
  SoftwareSigner? _soft;
  bool _destroyed = false;

  Future<SoftwareSigner> _signer() async {
    if (_destroyed) throw SignerDestroyedException();
    if (_soft case final s?) return s;
    final stored = await _storage.read(key: key);
    final List<int> seed;
    if (stored != null) {
      seed = base64Decode(stored);
    } else {
      final rng = Random.secure();
      seed = [for (var i = 0; i < 32; i++) rng.nextInt(256)];
      await _storage.write(key: key, value: base64Encode(seed));
    }
    return _soft = SoftwareSigner.fromSeed(seed, backend: backend);
  }

  @override
  Future<Uint8List> publicKey() async => (await _signer()).publicKey();

  @override
  Future<Uint8List> sign(Uint8List message) async =>
      (await _signer()).sign(message);

  @override
  Future<void> destroy() async {
    _destroyed = true;
    await _soft?.destroy();
    _soft = null;
    await _storage.delete(key: key);
  }
}
