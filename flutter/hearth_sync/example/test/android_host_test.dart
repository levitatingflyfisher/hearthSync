// Android host config that no widget test can see. The manifest names the
// activity relative to the Gradle namespace (".MainActivity"), so the class must
// live in that package or the release APK dies before Flutter starts
// (ClassNotFoundException, no frame). Seen on the emulator.
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

void main() {
  test('MainActivity lives in the Gradle namespace', () {
    final gradle = File('android/app/build.gradle.kts').readAsStringSync();
    final namespace = RegExp(
      r'namespace\s*=\s*"([^"]+)"',
    ).firstMatch(gradle)!.group(1)!;
    final manifest = File(
      'android/app/src/main/AndroidManifest.xml',
    ).readAsStringSync();
    expect(manifest, contains('android:name=".MainActivity"'));

    final declared = [
      for (final f in Directory('android/app/src/main').listSync(
        recursive: true,
      ))
        if (f is File && RegExp(r'MainActivity\.(kt|java)$').hasMatch(f.path))
          RegExp(
            r'^package\s+([\w.]+)',
            multiLine: true,
          ).firstMatch(f.readAsStringSync())!.group(1)!,
    ];
    expect(declared, [namespace]);
  });
}
