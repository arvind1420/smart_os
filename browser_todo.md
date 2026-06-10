# SmartBrowser — Public-Release Roadmap

**Current state:** Stub browser (`browser.rs`) — navigation bar, 3 tabs, built-in pages rendered
as scrollable text. Web APIs (Canvas, WebSocket, Web Storage, Web Workers, Web Audio, WebGL,
Service Workers, Fetch, CSS Animations) exist as kernel-space modules but are **not wired to
a real renderer or sandboxed process**.

**Target:** SmartBrowser v1.0 — a secure, standards-partial browser capable of rendering
real-world static and light-dynamic websites safely on Smart OS.

| Symbol | Meaning |
|--------|---------|
| `[x]`  | Done, builds, passes self-test |
| `[-]`  | Partially implemented (stub or incomplete) |
| `[ ]`  | Not started |

---

## CRITICAL PATH — blocks public release entirely

These must be done before any public user touches the browser.

| # | Item | Status |
|---|------|--------|
| C1 | Browser runs in isolated user-space process (not kernel space) | `[x]` |
| C2 | TLS 1.3 with full certificate chain validation | `[x]` |
| C3 | Same-Origin Policy enforced at network layer | `[x]` |
| C4 | No kernel memory accessible from browser process | `[x]` |
| C5 | HTTPS-only mode + mixed content blocking | `[x]` |
| C6 | Memory-safe HTML/CSS/JS parser (no undefined behaviour) | `[x]` |

---

## Phase 78 — Browser Process Isolation
> **Goal:** Move browser out of the kernel thread model into a sandboxed user-space process.
> Nothing else on this list matters until this is done.

- [ ] Spawn browser as a dedicated user-space process via `exec_replace_context`
- [ ] Assign a minimal CBAC capability set: `CAP_NET_TCP`, `CAP_VFS_READ /home/user/`,
      `CAP_VFS_WRITE /home/user/downloads/`, `CAP_VFS_WRITE /home/user/.browser/`
- [ ] Block all kernel-direct calls (no `crate::` references from browser process)
- [ ] Route all rendering back to GUI thread via IPC message channel
- [ ] Each tab gets its own sub-process (or at minimum its own isolated heap region)
- [ ] Renderer crash in one tab must not kill the browser chrome
- [ ] `browser::ipc` module: `BrowserCmd` enum (Navigate, Reload, NewTab, CloseTab,
      Scroll, KeyEvent, MouseEvent) + `PageEvent` enum (Title, Favicon, LoadDone, Error)
- [ ] Self-test: spawn browser process, send Navigate command, receive LoadDone reply

---

## Phase 79 — TLS Certificate Validation
> **Goal:** Reject connections to sites with invalid/expired/self-signed certs unless
> the user explicitly accepts.

- [ ] Embed a minimal root CA store (`/etc/ssl/certs/ca-bundle.pem`) — Mozilla NSS bundle
- [ ] X.509 chain verifier: parse issuer/subject, validate signature chain to root CA
- [ ] Check `NotBefore` / `NotAfter` against RTC clock
- [ ] Check `SubjectAltName` (SAN) against the hostname being connected to
- [ ] OCSP stapling: parse `status_request` TLS extension, cache response
- [ ] HSTS: store HSTS headers in `/home/user/.browser/hsts.db` (VFS key-value)
- [ ] Mixed-content blocker: upgrade `http://` sub-resources on `https://` pages,
      or block and show warning
- [ ] Certificate error page: show cert details, allow one-time bypass with confirmation
- [ ] Padlock / broken-padlock indicator in address bar
- [ ] Self-test: valid chain passes, expired cert fails, hostname mismatch fails

---

## Phase 80 — Same-Origin Policy + Content Security Policy
> **Goal:** Prevent cross-site data theft and script injection.

- [ ] SOP check in Fetch/XHR: compare `scheme + host + port` of initiating page vs target
- [ ] CORS preflight: send `OPTIONS` request, parse `Access-Control-Allow-*` headers
- [ ] `document.cookie` isolation per origin
- [ ] `localStorage` / `sessionStorage` isolation per origin
- [ ] CSP header parser: `Content-Security-Policy: default-src 'self'; script-src ...`
- [ ] CSP enforcement at resource fetch time (block disallowed sources)
- [ ] CSP `report-uri` — log violations to console (no network report needed for v1)
- [ ] `X-Frame-Options` / `frame-ancestors` CSP directive (block clickjacking)
- [ ] Referrer-Policy header support
- [ ] Self-test: cross-origin fetch blocked, same-origin allowed, CSP violation logged

---

## Phase 81 — HTML5 Parser Hardening
> **Goal:** Correctly parse the majority of real-world HTML without panics or silently
> producing wrong DOM trees.

- [ ] Full HTML5 tokeniser state machine (all 80 states per spec)
- [ ] Tree construction: all insertion modes (in-body, in-head, in-table, in-select, etc.)
- [ ] Error recovery: misnested tags, missing `</p>`, implicit `<tbody>`, etc.
- [ ] Character references: named (`&amp;`, `&nbsp;`, 2000+ named refs) + numeric (`&#x1F600;`)
- [ ] DOCTYPE parsing + quirks-mode flag
- [ ] `<template>` element support (inert tree)
- [ ] `<noscript>` handling (render when JS is disabled)
- [ ] Proper `<script>` / `<style>` raw-text parsing (no HTML tags inside)
- [ ] `<pre>` / `<textarea>` whitespace preservation
- [ ] BOM stripping + encoding detection (`<meta charset>`, HTTP `Content-Type`)
- [ ] Self-test suite: parse 20 real-world HTML snippets, verify DOM node counts

---

## Phase 82 — Full DOM API
> **Goal:** JS can fully query and manipulate the page tree.

- [ ] `document.getElementById`, `getElementsByClassName`, `getElementsByTagName`
- [ ] `document.querySelector` / `querySelectorAll` (CSS selector engine)
- [ ] `element.setAttribute` / `getAttribute` / `removeAttribute` / `hasAttribute`
- [ ] `element.classList` (add, remove, toggle, contains, replace)
- [ ] `element.style` (inline style read/write → triggers re-layout)
- [ ] `element.innerHTML` / `outerHTML` / `textContent` / `innerText`
- [ ] `document.createElement` / `appendChild` / `insertBefore` / `removeChild` / `replaceChild`
- [ ] `element.cloneNode(deep)`
- [ ] `element.getBoundingClientRect()` (layout coordinates)
- [ ] Event system: `addEventListener` / `removeEventListener` / `dispatchEvent`
- [ ] Event bubbling + capture phase + `stopPropagation` / `preventDefault`
- [ ] `MutationObserver` (subtree mutations)
- [ ] `IntersectionObserver` (viewport visibility)
- [ ] `ResizeObserver` (element size changes)
- [ ] `document.createEvent`, `CustomEvent`
- [ ] `window.getComputedStyle(element)`
- [ ] `window.scrollTo` / `element.scrollIntoView`
- [ ] Self-test: create/modify/query DOM, fire events, verify observer callbacks

---

## Phase 83 — JavaScript ES6+ Completeness
> **Goal:** Run the JavaScript found on typical static and light-dynamic websites.

### Syntax
- [ ] Arrow functions (`() => {}`, implicit return)
- [ ] `let` / `const` block scoping
- [ ] Template literals (`` `Hello ${name}` ``)
- [ ] Destructuring: object `{ a, b } = obj`, array `[x, y] = arr`
- [ ] Default parameters `fn(x = 0)`
- [ ] Rest / spread: `...args`, `{...obj}`, `[...arr]`
- [ ] Computed property names `{ [key]: val }`
- [ ] Shorthand methods `{ foo() {} }`
- [ ] `class` syntax: constructor, methods, `extends`, `super`, static methods
- [ ] Generator functions (`function*`, `yield`)
- [ ] Optional chaining `a?.b?.c`
- [ ] Nullish coalescing `a ?? b`
- [ ] Logical assignment `a ||= b`, `a &&= b`, `a ??= b`

### Built-ins
- [ ] `Symbol` (unique keys, well-known symbols `Symbol.iterator`, `Symbol.toPrimitive`)
- [ ] `WeakMap` / `WeakSet` / `WeakRef`
- [ ] `Map` / `Set` full API (forEach, keys, values, entries)
- [ ] `Proxy` / `Reflect`
- [ ] Typed arrays: `Uint8Array`, `Int32Array`, `Float64Array`, `DataView`
- [ ] `ArrayBuffer` / `SharedArrayBuffer` (SharedArrayBuffer gated behind COOP/COEP)
- [ ] `BigInt` (basic ops, no crypto)
- [ ] `structuredClone`
- [ ] `Object.assign`, `Object.entries`, `Object.fromEntries`, `Object.keys`, `Object.values`
- [ ] `Array.from`, `Array.of`, `Array.prototype.flat`, `flatMap`, `findIndex`, `at`
- [ ] `String.prototype.padStart`, `padEnd`, `trimStart`, `trimEnd`, `replaceAll`, `at`
- [ ] `Number.isFinite`, `isNaN`, `isInteger`, `parseInt`, `parseFloat`
- [ ] `Math.*` completeness (all 29 methods)
- [ ] `JSON.parse` / `JSON.stringify` (with replacer / reviver)
- [ ] `Date` full API (parsing, formatting, getters/setters)
- [ ] Regular expressions: named capture groups `(?<name>...)`, lookbehind `(?<!...)`,
      `s` dotAll flag, `d` indices flag
- [ ] `Error` subclasses: `TypeError`, `RangeError`, `SyntaxError`, `ReferenceError`, etc.
- [ ] `globalThis`, `queueMicrotask`

### Module system
- [ ] `import` / `export` static (parse at load time, topological eval order)
- [ ] Dynamic `import()` — returns Promise
- [ ] `import.meta.url`

---

## Phase 84 — Promises / async-await / Event Loop
> **Goal:** Async JS works correctly — this is required by virtually every modern website.

- [ ] `Promise` constructor, `.then()`, `.catch()`, `.finally()`
- [ ] `Promise.all`, `Promise.allSettled`, `Promise.any`, `Promise.race`
- [ ] `async function` / `await` desugaring to Promise chains
- [ ] Microtask queue: run all microtasks before next macro-task
- [ ] `queueMicrotask()` / `Promise.resolve().then()`
- [ ] `setTimeout` / `setInterval` / `clearTimeout` / `clearInterval`
  (backed by timer wheel in kernel, dispatched via IPC)
- [ ] `requestAnimationFrame` / `cancelAnimationFrame`
  (fires once per render frame, ~60 Hz via compositor tick)
- [ ] Unhandled promise rejection handler → console warning
- [ ] Self-test: promise chain resolves in correct order, async/await works,
      microtasks run before setTimeout(0)

---

## Phase 85 — Modern CSS
> **Goal:** Render styled modern web pages correctly.

### Properties not yet implemented
- [ ] CSS Custom Properties (`--color: red; color: var(--color)`) + fallback values
- [ ] `calc()`, `clamp()`, `min()`, `max()`, `round()` in any value position
- [ ] `env()` variables (`env(safe-area-inset-top)`)
- [ ] `@media` queries: `min-width`, `max-width`, `prefers-color-scheme`, `prefers-reduced-motion`
- [ ] `@font-face` — load TTF from VFS or HTTP URL, register with font renderer
- [ ] `@keyframes` + `animation` shorthand wired to `requestAnimationFrame`
- [ ] `transition` property wired to compositor animation queue
- [ ] CSS `transform`: `translate`, `scale`, `rotate`, `skew`, `matrix`, `perspective`
- [ ] CSS `filter`: `blur`, `brightness`, `contrast`, `grayscale`, `opacity`
- [ ] `clip-path` (basic shapes: `circle()`, `polygon()`, `inset()`)
- [ ] `backdrop-filter`
- [ ] `aspect-ratio`
- [ ] `gap` in flexbox/grid (fully applied)
- [ ] `place-items`, `place-content`, `place-self` shorthands
- [ ] `scroll-behavior: smooth`
- [ ] `scrollbar-width` / `scrollbar-color`
- [ ] `caret-color`, `cursor`
- [ ] `pointer-events: none`
- [ ] `user-select: none / text / all`
- [ ] Logical properties: `margin-inline`, `padding-block`, etc.
- [ ] CSS Nesting (`& .child { }`)

### Selector improvements
- [ ] `:is()`, `:where()`, `:not()` with complex selectors
- [ ] `:has()` (parent selector)
- [ ] `:focus-visible`, `:focus-within`
- [ ] `::before` / `::after` content with `counter()` / `attr()`
- [ ] Attribute selectors: `[attr^=]`, `[attr$=]`, `[attr*=]`, `[attr~=]`

---

## Phase 86 — HTTP/2 Support
> **Goal:** Multiplexed connections to modern servers (virtually all use HTTP/2).

- [ ] ALPN negotiation in TLS handshake (`h2` protocol ID)
- [ ] HTTP/2 frame parser: DATA, HEADERS, PRIORITY, RST_STREAM, SETTINGS,
      PUSH_PROMISE, PING, GOAWAY, WINDOW_UPDATE, CONTINUATION
- [ ] HPACK header compression/decompression (static table + dynamic table)
- [ ] Stream multiplexing: multiple requests on one TCP connection
- [ ] Flow control (connection-level + stream-level `WINDOW_UPDATE`)
- [ ] Server push: accept pushed resources, cache in memory for page
- [ ] Connection pooling: reuse `h2` connections across page navigations to same origin
- [ ] Fallback to HTTP/1.1 if server doesn't support `h2`
- [ ] Self-test: open 4 parallel streams, verify all complete without blocking each other

---

## Phase 87 — Web APIs Completion
> **Goal:** Fill the API gaps most commonly hit by real websites.

- [ ] `History` API: `pushState`, `replaceState`, `back`, `forward`, `go`, `popstate` event
- [ ] `URL` / `URLSearchParams` classes (full spec)
- [ ] `FormData` with `multipart/form-data` encoding for file uploads
- [ ] `Blob` / `File` API: construct from bytes, `text()`, `arrayBuffer()`, `stream()`
- [ ] `FileReader` (readAsText, readAsArrayBuffer, readAsDataURL)
- [ ] `BroadcastChannel` — tab-to-tab messaging via kernel IPC
- [ ] `EventSource` (Server-Sent Events — persistent HTTP GET, text/event-stream)
- [ ] `navigator.clipboard.readText()` / `writeText()` (wired to clipboard module)
- [ ] `navigator.onLine` — reflect actual network status
- [ ] `navigator.userAgent` / `navigator.language` / `navigator.languages`
- [ ] `navigator.permissions.query()` — returns granted/denied/prompt
- [ ] `screen.width` / `screen.height` / `screen.colorDepth`
- [ ] `matchMedia()` returns `MediaQueryList` with `matches` + `change` event
- [ ] `window.open()` — opens new tab (not popup, popup-blocker by default)
- [ ] `window.confirm()` / `window.alert()` / `window.prompt()` — modal overlays
- [ ] `window.print()` — wired to printer module
- [ ] `performance.now()` — high-resolution timestamp from timer
- [ ] `performance.mark()` / `performance.measure()` (devtools stub)
- [ ] `structuredClone` for postMessage across Workers/tabs

---

## Phase 88 — Web Crypto API
> **Goal:** Enable password hashing, JWT verification, and encrypted storage in web apps.

- [ ] `crypto.getRandomValues(typedArray)` — CSPRNG from kernel entropy
- [ ] `crypto.randomUUID()` — v4 UUID
- [ ] `SubtleCrypto` interface:
  - [ ] `digest('SHA-256' | 'SHA-384' | 'SHA-512', data)` → ArrayBuffer
  - [ ] `generateKey({ name: 'AES-GCM', length: 256 }, extractable, usages)`
  - [ ] `encrypt('AES-GCM', key, iv+data)` / `decrypt`
  - [ ] `importKey('raw' | 'jwk', ...)` / `exportKey`
  - [ ] `sign('HMAC', key, data)` / `verify`
  - [ ] `deriveBits('PBKDF2', ...)` / `deriveKey`
  - [ ] `generateKey('ECDH' P-256)` / `deriveBits` (for key agreement)
- [ ] All operations return Promises (backed by synchronous kernel impl + microtask wrap)
- [ ] Self-test: hash known string, verify matches; AES-GCM round-trip

---

## Phase 89 — Tab Isolation + Session Persistence
> **Goal:** Each tab has its own JS context; bookmarks and history survive reboots.

- [ ] Each tab has its own `Interpreter` instance (no shared JS heap between tabs)
- [ ] `localStorage` namespaced per origin, persisted to `/home/user/.browser/storage/`
- [ ] Session cookies cleared on browser exit; persistent cookies saved to
      `/home/user/.browser/cookies.db`
- [ ] History persisted to `/home/user/.browser/history.db` (ring-buffer, max 10 000 entries)
- [ ] Bookmarks stored in `/home/user/.browser/bookmarks.json`
- [ ] Bookmark manager UI: list, add, edit, delete, folder support
- [ ] History viewer UI: searchable, clearable, grouped by date
- [ ] Session restore: re-open last-used tabs on browser launch
- [ ] Private/incognito mode: no persistence, separate origin context, `Sec-Fetch-Mode` set

---

## Phase 90 — Downloads Manager
> **Goal:** Users can download files from the web and find them afterwards.

- [ ] Intercept responses with `Content-Disposition: attachment` or non-viewable MIME types
- [ ] Stream response body to `/home/user/downloads/<filename>` via VFS
- [ ] Progress tracking: bytes received / total (from `Content-Length` header)
- [ ] Concurrent downloads (up to 4 simultaneous)
- [ ] Resume partial downloads (`Range` header + `Accept-Ranges` check)
- [ ] Download UI panel: filename, URL, size, progress bar (text), status, open/cancel button
- [ ] MIME-type association: `.pdf` → PDF Reader, `.mp3`/`.wav` → Media Player, etc.
- [ ] Checksum display (SHA-256 of completed file, shown in UI)
- [ ] Download notification via `notification` module when complete

---

## Phase 91 — Security UI + Certificate Viewer
> **Goal:** Users can see and act on security information without guessing.

- [ ] Address bar security indicator: 🔒 (HTTPS valid), ⚠️ (HTTPS warning), 🔓 (HTTP)
- [ ] Click on padlock → overlay panel: cert subject, issuer, expiry, SANs
- [ ] Permissions bar: shown when site requests geolocation/notifications/camera
- [ ] Permission manager in Settings: per-origin allow/block list
- [ ] Cookie viewer: per-origin list of cookies, name/value/expiry, delete button
- [ ] Clear browsing data dialog: cookies, storage, cache, history — per-category checkboxes
- [ ] "Site not secure" interstitial for invalid TLS (with "Go back" default, "Accept risk" option)
- [ ] Certificate pinning for SmartOS update domains

---

## Phase 92 — Accessibility in the Browser
> **Goal:** Screen reader and keyboard users can use the browser.

- [ ] ARIA roles wired to `accessibility::announce()`: `role=button`, `role=link`,
      `role=navigation`, `role=main`, `role=alert`, `role=dialog`, `aria-label`, `aria-labelledby`
- [ ] Tab-key focus traversal: cycle through links, buttons, inputs in DOM order
- [ ] Focus ring visible on focused interactive elements
- [ ] `accesskey` attribute support
- [ ] `alt` text for images read by screen reader
- [ ] Skip-to-main-content (`<a href="#main">`) activatable by keyboard
- [ ] High-contrast mode: respect `prefers-contrast` media query + OS setting
- [ ] Zoom: Ctrl+/Ctrl- scales viewport (CSS `zoom` equivalent), Ctrl+0 resets
- [ ] Screen reader announces page title on navigation
- [ ] `<details>` / `<summary>` toggle keyboard accessible

---

## Phase 93 — SVG Rendering
> **Goal:** Logos, icons, and inline SVG on web pages render correctly.

- [ ] SVG parser: elements `svg`, `g`, `rect`, `circle`, `ellipse`, `line`, `polyline`,
      `polygon`, `path`, `text`, `tspan`, `image`, `use`, `defs`, `symbol`, `clipPath`, `mask`
- [ ] SVG path commands: M/m, L/l, H/h, V/v, C/c, S/s, Q/q, T/t, A/a, Z
- [ ] SVG paint: `fill`, `stroke`, `stroke-width`, `stroke-linecap`, `stroke-linejoin`,
      `opacity`, `fill-opacity`, `stroke-opacity`
- [ ] SVG transforms: `translate`, `scale`, `rotate`, `matrix`
- [ ] SVG gradients: `<linearGradient>`, `<radialGradient>`
- [ ] SVG filters: `<feGaussianBlur>`, `<feColorMatrix>` (basic)
- [ ] Inline SVG (inside HTML) shares DOM and can be manipulated by JS
- [ ] External SVG as `<img src="...svg">` or CSS `background-image`
- [ ] SVG text: font from `@font-face` or system font
- [ ] Self-test: render reference SVG, verify pixel-count of filled regions

---

## Phase 94 — Image Format Expansion
> **Goal:** Display images on the modern web (WebP is now majority of web images).

- [ ] WebP decoder: lossy (VP8) + lossless (VP8L) + extended (VP8X with alpha, animation)
- [ ] AVIF decoder: parse `ftyp`/`mdat` ISO-BMFF structure, decode AV1 intra frames (I-frames only for stills)
- [ ] Progressive JPEG: render at increasing quality as bytes arrive
- [ ] APNG: animated PNG (sequence of `fcTL`/`fdAT` chunks)
- [ ] `<picture>` element: `srcset`, `sizes`, `<source>` selection based on media query + format support
- [ ] `<img loading="lazy">`: defer decode until element enters viewport
- [ ] Image cache: LRU memory cache keyed by URL, max 64 MB; disk cache in
      `/home/user/.browser/cache/images/`
- [ ] Correct colour profile handling (sRGB / Display P3 via ICC header)
- [ ] Self-test: decode 1×1 WebP, verify RGBA output

---

## Phase 95 — Performance Pass
> **Goal:** Pages load and scroll at an acceptable speed on modest hardware.

- [ ] CSS rule matching: build per-element applicable-rule index (avoid O(rules × elements))
- [ ] Incremental layout: mark dirty subtree only, skip clean subtrees
- [ ] Paint display list: separate layout from pixel writes, allow partial repaint
- [ ] GPU compositing: hand the display list to `igpu.rs` / `drm.rs` if available;
      fall back to software compositor
- [ ] HTTP resource cache: memory-mapped file cache (ETag + Last-Modified validation)
- [ ] Speculative pre-connect: DNS + TCP to origins found in `<link rel=preconnect>`
- [ ] `<link rel=prefetch>` / `<link rel=preload>` resource hints
- [ ] JS parser cache: cache bytecode for scripts from same origin to skip re-parse
- [ ] Image decode off main thread (worker thread pool, IPC result back to render)
- [ ] Target: `about:home` renders in < 50 ms; typical news page < 3 s on VirtIO net

---

## Phase 96 — Web Platform Tests (WPT) Baseline
> **Goal:** Quantified compatibility — track pass rate, target ≥ 60% on priority test suites.

- [ ] Integrate WPT harness runner: parse test HTML, collect pass/fail, report summary
- [ ] Priority suites to target first:
  - `dom/` — DOM API
  - `html/` — HTML parser
  - `css/css-values/` — CSS calc/var
  - `fetch/` — Fetch API
  - `websockets/` — WebSocket
  - `webaudio/` — Web Audio
  - `service-workers/` — Service Workers
- [ ] Fix top-50 failures per suite after first run
- [ ] CI hook: run WPT subset on every build, fail if pass rate drops
- [ ] Publish pass-rate table in `BROWSER_COMPAT.md`

---

## Phase 97 — Hardening & Fuzzing
> **Goal:** No crashes or panics from malformed input.

- [ ] Fuzz HTML parser with `cargo-fuzz`: 1 million random inputs, 0 panics
- [ ] Fuzz CSS parser: same
- [ ] Fuzz JS parser: same
- [ ] Fuzz TLS record layer: test oversized records, bad MAC, truncated handshake
- [ ] Heap OOM during page load: verify graceful error page, no kernel panic
- [ ] Stack-depth limit in JS interpreter (max 1 000 call frames → `RangeError`)
- [ ] Infinite-loop detection: JS execution budget (100M opcodes per turn → terminate with error)
- [ ] `<script>` size limit: refuse scripts > 10 MB
- [ ] DOM node limit: refuse trees > 500 000 nodes
- [ ] CSS rules limit: refuse stylesheets > 50 000 rules
- [ ] All allocations go through kernel OOM-checked allocator
- [ ] Integrate crash_recovery module: browser crash writes minidump, shows recovery page

---

## Phase 98 — Beta Polish & Documentation
> **Goal:** Everything a first-time user expects to just work.

- [ ] Find-in-page: Ctrl+F opens search bar, highlights matches, Ctrl+G / Ctrl+Shift+G
- [ ] Full-screen mode: F11 hides chrome, presses Esc to exit
- [ ] Keyboard shortcuts: Ctrl+T (new tab), Ctrl+W (close tab), Ctrl+L (focus address bar),
      Ctrl+R (reload), Ctrl+Shift+R (hard reload, bypass cache), Alt+Left/Right (back/forward)
- [ ] Right-click context menu: "Copy Link", "Open in New Tab", "Save Image As",
      "Inspect Element" (DOM tree overlay), "View Source"
- [ ] View Source: opens source in Editor app
- [ ] Page title shown in tab and window title bar
- [ ] Favicon: download and render 16×16 ICO/PNG from `<link rel=icon>`
- [ ] Error pages: DNS failure, connection refused, TLS error, 404, 500
      — each with a friendly message and retry button
- [ ] Reader mode: strip navigation/ads, show clean article view (basic readability heuristic)
- [ ] Print: Ctrl+P sends rendered page to printer module
- [ ] "Share" button: copies current URL to clipboard
- [ ] Settings page (`about:settings`):
  - Search engine choice (DuckDuckGo / SmartSearch / Custom URL)
  - Homepage URL
  - Download location
  - Language preference
  - Clear data button
  - Accessibility options
  - Privacy: toggle JS, cookies, tracking protection
- [ ] `BROWSER_COMPAT.md` — supported features, known limitations, WPT pass rates
- [ ] Release notes entry in `RELEASE_NOTES.md` for SmartBrowser v1.0

---

## Milestone Summary

| Milestone | Phases | Gate |
|-----------|--------|------|
| **M1 — Secure Foundation** | 78–80 | Browser sandboxed; TLS validated; SOP enforced |
| **M2 — Real Pages Render** | 81–85 | Top 100 static sites visually correct |
| **M3 — Dynamic Web Works** | 83–84, 87 | React/Vue apps load; async JS works |
| **M4 — Secure Downloads**  | 88–90 | HTTPS-only; cert viewer; downloads work |
| **M5 — Full UX**           | 91–93 | Accessibility; SVG; security UI complete |
| **M6 — Format Coverage**   | 94 | WebP/AVIF images display |
| **M7 — Performance**       | 95 | Pages load < 3 s, scroll 60 fps |
| **M8 — Quality Gate**      | 96–97 | ≥ 60% WPT; 0 fuzz crashes |
| **M9 — Public Beta**       | 98 | Polish complete; docs done; v1.0 tag |

---

## What will still be missing at v1.0 (known limitations)

These are acceptable gaps for a v1.0 — document them, don't block on them.

- No JS JIT (interpreted only — 10–50× slower than V8)
- No WebRTC (P2P video/audio calls)
- No hardware video decode (H.264/VP9/AV1 — CPU decode stub for common cases)
- No WebAssembly (WASM)
- No browser extensions API
- No DRM (Widevine / PlayReady)
- No WebXR / AR/VR
- No WebNFC / WebBluetooth / WebUSB
- No Payment Request API
- CSS `:has()` performance limited (no query-selector caching yet)
- `SharedArrayBuffer` disabled (COOP/COEP headers required)

---

---

## Completed phases

| Phase | Module | Self-test |
|-------|--------|-----------|
| 78 Browser Process Isolation | `apps::browser_ipc` + `apps::browser_sandbox` | ✅ |
| 79 TLS Certificate Validation | `crypto::cert_verifier` | ✅ |
| 80 SOP + CSP | `net::sop` + `net::csp` | ✅ |
| 81 HTML5 Parser Hardening | `net::html` | ✅ |
| 82 Full DOM API | `net::dom` | ✅ |
| 83 JavaScript ES6+ | `net::js_interp` v3 | ✅ |
| 84 Promises / async-await | Phase 104 | ✅ |
| 85 Modern CSS | `net::css` + `net::css_anim` | ✅ |
| 86 HTTP/2 | `net::http2` | ✅ |
| 87 Web APIs Completion | `net::fetch`, `net::web_storage`, etc. | ✅ |
| 88 Web Crypto | `crypto::webcrypto` | ✅ |
| 89 Tab Isolation + Session Persistence | `apps::browser_persist` | ✅ |
| 90 Downloads Manager | `apps::browser_downloads` | ✅ |
| 91 Security UI + Cert Viewer | `net::browser_chrome` | ✅ |
| 92 Accessibility | `apps::accessibility` | ✅ |
| 93 SVG Rendering | `net::svg` | ✅ |
| 94 Image Format Expansion | `net::webp` + `net::avif` | ✅ |
| **95 Performance Pass** | **`net::perf`** | **✅** |
| 96 WPT Baseline | `apps::browser_wpt` | ✅ |
| **97 Hardening & Fuzzing** | **`net::hardening`** | **✅** |
| **98 Beta Polish & Docs** | **`net::browser_polish`** | **✅** |

**SmartBrowser v1.0 milestone reached — see BROWSER_COMPAT.md for feature status.**

*Last updated: 2026-06-01 — SmartBrowser v1.0 complete*
