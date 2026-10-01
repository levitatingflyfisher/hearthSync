// Off the web there is no browser probe.

/// Starts the browser device probe when the page URL asks for one (`?device=`);
/// null when it does not, and always off the web.
Future<String?> startProbe() async => null;
