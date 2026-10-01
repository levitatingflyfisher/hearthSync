// Which relay URLs RelayClient accepts. dart:io's HttpClient ignores Android's
// cleartext policy (seen on the emulator: a plain http://10.0.2.2 relay synced
// with no network-security config), so the client itself is the only thing that
// can refuse cleartext. Envelopes are sealed and signed either way, but channel
// ids, timing and sizes would cross the network in the clear.
import 'package:flutter_test/flutter_test.dart';
import 'package:hearth_sync/hearth_sync.dart';

void main() {
  group('RelayClient accepts', () {
    for (final url in [
      'https://relay.example/',
      'https://relay.example:8443/sub/',
      'HTTPS://Relay.Example/',
      'http://127.0.0.1:18081/',
      'http://localhost:18081/',
      'http://LOCALHOST/',
      'http://[::1]:18081/',
      'http://10.0.2.2:18081/',
    ]) {
      test(url, () {
        expect(() => RelayClient(Uri.parse(url)), returnsNormally);
      });
    }
  });

  group('RelayClient refuses', () {
    for (final url in [
      'http://relay.example/',
      'http://192.168.1.20:18081/',
      'http://10.0.2.3:18081/',
      'http://127.0.0.2:18081/',
      'http://localhost.relay.example/',
      'http://[::2]/',
      'ws://relay.example/',
      'file:///relay',
      'https:///nohost',
    ]) {
      test(url, () {
        expect(
          () => RelayClient(Uri.parse(url)),
          throwsA(
            isA<ArgumentError>().having(
              (e) => e.message,
              'message',
              contains('https://'),
            ),
          ),
        );
      });
    }
  });
}
