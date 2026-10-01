// Off native platforms there is no device panel (the web has the browser probe).
import 'package:flutter/widgets.dart';

/// The on-device probe panel (lib/device/panel_io.dart), or null.
Widget? devicePanel() => null;
