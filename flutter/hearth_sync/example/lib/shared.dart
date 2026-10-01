// What the demo page and the browser probe share.
import 'dart:typed_data';

import 'package:hearth_sync/hearth_sync.dart';

/// The demo's schema: one table and one set.
const demoSchema = SyncSchema([
  SyncTable('rooms', [SyncField('name', SyncType.text)]),
  SyncSet('groceries', SyncType.text),
]);

/// A demo household seed. A real app derives it from the 12 words.
final demoSeed = Uint8List.fromList(List.generate(64, (i) => i));
