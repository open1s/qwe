# RFC-0041: Self-contained offline 3D viewer (vendored three.js)
**Status:** Normative. The reference 3D viewer (`pwe present`) is **fully
self-contained and offline**: three.js and its addons are vendored into the
repository, embedded in the binary, and served from the local origin — never
loaded from a CDN. The viewer also releases its GPU/DOM resources promptly on
teardown and per frame.

## Motivation

Loading three.js from a CDN via an import map fails closed in an offline or
air-gapped environment: an unreachable import leaves the browser tab pending
forever (a blank, hanging viewer). A reference toolchain must run with no
network. Additionally, a long-lived live view that allocates geometry,
materials, textures, and DOM nodes each frame leaks GPU/CPU memory without
explicit disposal.

## Design

### Vendoring and serving

- `reference/vendor/three/three.module.js`, `reference/vendor/three/addons/…`,
  and `reference/vendor/three/LICENSE` (three.js r160, MIT) are committed.
- `present::vendor_file(path)` (`include_str!`) embeds them in the binary; the
  live HTTP server serves them at `/vendor/three/…`.
- The viewer page's import map points only at `/vendor/three/…` (local origin);
  there is no remote import map or CDN reference.

### Teardown

- A `pagehide` handler aborts any in-flight `/state` fetch and releases the
  WebGL context (`renderer.dispose()` + `forceContextLoss()`).
- No `beforeunload` handler (it delays navigation); the page pauses on
  `visibilitychange` when hidden.

### Per-frame resource lifecycle

- `disposeObj(o)`/`disposeMat(m)` free `geometry`, `material` (and its textures),
  and remove the object's DOM element (labels) before dropping references;
  entity meshes, field isosurfaces, lines, points, decals, and removed entities
  are disposed on change.
- `renderer.info.memory` stays flat during a live run (verified by soak).

## Validation

- `reference/src/present.rs` unit tests assert the served viewer references only
  `/vendor/three/…` and that `vendor_file` resolves the modules and LICENSE.
- Manual soak: a live run holds a flat `renderer.info.memory` and RSS.

## Consequences

- Works offline/air-gapped; deterministic assets (pinned version).
- Repository carries ~1 MB of vendored JS (a deliberate, bounded trade-off).
