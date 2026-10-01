{{flutter_js}}
{{flutter_build_config}}
// Flutter's default bootstrap, plus a config that keeps every engine fetch on
// this origin: CanvasKit from the copy the build puts in canvaskit/, and the
// engine's fallback fonts from a local path rather than Google's CDN. The demo
// bundles a family named Roboto, so the engine's default font is never
// fetched; a glyph Roboto lacks would ask fallback-fonts/ and 404 here.
_flutter.loader.load({
  serviceWorkerSettings: {
    serviceWorkerVersion: {{flutter_service_worker_version}}
  },
  config: {
    canvasKitBaseUrl: "canvaskit/",
    fontFallbackBaseUrl: "fallback-fonts/"
  }
});
