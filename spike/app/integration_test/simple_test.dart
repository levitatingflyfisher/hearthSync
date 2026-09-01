import 'package:flutter_test/flutter_test.dart';
import 'package:app/main.dart';
import 'package:app/src/rust/frb_generated.dart';
import 'package:integration_test/integration_test.dart';

// On-device check (emulator/browser); not runnable on this box's headless host.
void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  setUpAll(() async => await RustLib.init());
  testWidgets('op vector id matches over the bridge', (tester) async {
    await tester.pumpWidget(const MyApp());
    expect(find.textContaining('Vector: PASS'), findsOneWidget);
  });
}
