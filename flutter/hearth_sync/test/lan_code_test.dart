// The pairing code and the LAN keys: pure Dart, no bridge, no sockets.
import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:hearth_sync/hearth_sync.dart';

void main() {
  group('LanCode', () {
    final code = LanCode('192.168.1.23', 48123, Uint8List.fromList([9, 8, 7, 6]));

    test('round-trips through its text, three groups of six', () {
      expect(code.text, matches(RegExp(r'^[0-9A-Z]{6}-[0-9A-Z]{6}-[0-9A-Z]{6}$')));
      final back = LanCode.tryParse(code.text)!;
      expect(back.host, '192.168.1.23');
      expect(back.port, 48123);
      expect(back.token, [9, 8, 7, 6]);
    });

    test('reads what people type: lower case, spaces, O for 0, I and L for 1', () {
      final typed = code.text
          .toLowerCase()
          .replaceAll('-', ' ')
          .replaceAll('0', 'o')
          .replaceAll('1', 'l');
      expect(LanCode.tryParse(' $typed ')?.text, code.text);
    });

    test('a typo fails at parse, not at connect', () {
      final t = code.text;
      for (var i = 0; i < t.length; i++) {
        if (t[i] == '-') continue;
        final swapped = t.replaceRange(i, i + 1, t[i] == 'A' ? 'B' : 'A');
        expect(LanCode.tryParse(swapped), isNull, reason: 'changed char $i');
      }
      expect(LanCode.tryParse(t.substring(0, 17)), isNull);
      expect(LanCode.tryParse(''), isNull);
      expect(LanCode.tryParse('hello'), isNull);
    });

    test('a URI carrying the code is read too', () {
      expect(LanCode.tryParse('lullaby://wifi-sync?code=${code.text}')?.port, 48123);
    });
  });

  group('pickLanAddress', () {
    test('prefers the Wi-Fi interface’s private address', () {
      expect(
        pickLanAddress({
          'rmnet_data0': ['100.81.4.2'],
          'tailscale0': ['100.101.7.9'],
          'wlan0': ['192.168.4.37'],
        }),
        '192.168.4.37',
      );
    });

    test('skips loopback, link-local, carrier-grade NAT and cellular', () {
      expect(
        pickLanAddress({
          'lo': ['127.0.0.1'],
          'wlan0': ['169.254.3.3'],
          'tun0': ['100.64.0.5'],
          'rmnet0': ['10.140.2.2'],
        }),
        isNull,
      );
    });

    test('takes an Ethernet or emulator address when there is no Wi-Fi', () {
      expect(pickLanAddress({'eth0': ['10.0.2.15']}), '10.0.2.15');
      expect(pickLanAddress({'enp3s0': ['172.20.1.4']}), '172.20.1.4');
    });
  });

  group('LanKeys', () {
    test('derive the known answer (vectors computed with Python’s hmac)', () async {
      final seed = Uint8List.fromList(List.generate(64, (i) => i));
      final keys = await LanKeys.derive(seed, 'lullaby', Uint8List.fromList([1, 2, 3, 4]));
      expect(
        hex(keys.household),
        'a3e1461fe93763737dd2adf41d478734782a4136cf8b8c4ad8ee382043423d21',
      );
      expect(
        hex(keys.token),
        '868ea0ed9012537e2fda195d360f06915f97809ca8a1f9c40facb93470b1c567',
      );
    });
  });
}
