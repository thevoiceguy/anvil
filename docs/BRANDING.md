# Anvil — FCP brand provisioning

This is the contract between Anvil (the softphone) and FCP (the server). A company
deploying FCP configures a **brand profile** — app name, logo, colors, ringtones,
support links — and Anvil downloads it on login so the UI theme matches the tenant.

The goal is that a user installs one generic Anvil build and, on first login to
`acme-corp.example.com`, the app re-shells into Acme's identity without any custom
binary being produced.

---

## 1. Lifecycle

```
App launch ──► (unbranded login screen) ──►
  user enters AOR + password ──►
  Anvil starts:
    1. Resolve provisioning URL (see §2)
    2. Load cached BrandProfile if any (by tenant) ──► emit BrandUpdated
    3. Fetch fresh profile with If-None-Match: <cached etag>
       - 200 OK → new profile, emit BrandUpdated, persist
       - 304 Not Modified → keep cached
    4. Download any new assets (logo, icon, ringtones) referenced by sha256
    5. REGISTER
```

Steps 2–4 happen concurrently with REGISTER when the cache is warm, so there's
no added latency for returning users. A cold login waits for the fetch before
REGISTER so the first UI paint is branded.

The UI subscribes to `Event::BrandUpdated { profile }` and re-renders. A profile
may arrive twice per login (cached, then fresh) — UIs must be idempotent.

---

## 2. Provisioning URL discovery

In order, Anvil tries:

1. **Explicit:** `AccountConfig.provisioning_url`. Set by the user in settings or
   baked into a pre-configured install.
2. **Header:** `X-FCP-Provisioning-Url` on the REGISTER 200 OK. Lets FCP push the
   URL without the user typing it — they only type their SIP credentials.
3. **Well-known:** `https://{registrar-host}/.well-known/fcp-provisioning`.
   Tried last because some registrars sit behind SIP-only load balancers that
   don't serve HTTP.

If all three fail, branding is skipped and the UI renders with its built-in
default theme. This is not an error.

---

## 3. Endpoint

```
GET {provisioning_url}/brand/v1
Host: {provisioning host}
Accept: application/json
Authorization: {see §4}
If-None-Match: "{etag}"   (optional, when client has a cached copy)
User-Agent: Anvil/0.1 (linux)
```

Query parameters:

| Name     | Required | Description                                                       |
|----------|----------|-------------------------------------------------------------------|
| `aor`    | yes      | URL-encoded SIP AOR of the logged-in user                         |
| `locale` | no       | BCP 47 tag, e.g. `en-US`; server may localize `app_name` / links   |
| `theme`  | no       | `light` or `dark`; hint only — response always includes both       |

Response on 200 OK: `Content-Type: application/json`, `ETag: "..."` (required),
body per §5.

Response on 304 Not Modified: empty body, no `ETag` refresh required.

Response on 401 Unauthorized: client drops cached profile, emits `Event::Error`,
UI reverts to default theme.

Response on 404 Not Found: tenant has no custom branding; same fallback as 401
but without the error event.

---

## 4. Authentication

Anvil sends whatever the `BrandCredential` in `BrandConfig` says. Three modes:

| Mode      | Header                                                      | Typical use                                       |
|-----------|-------------------------------------------------------------|---------------------------------------------------|
| `Bearer`  | `Authorization: Bearer <token>`                             | FCP issues a short-lived token after SIP auth     |
| `Basic`   | `Authorization: Basic base64(username:password)`            | Reuse SIP credentials (simple, dev-only)          |
| `None`    | (omitted)                                                   | Public branding (e.g. marketing preview tenants)  |

FCP takes only `Bearer` (its `docs/BRANDING.md`): the app's own session from
signing in (FCP's `docs/APP_SIGN_IN.md`). `anvil-fcp`'s `FcpBrandProvider`
signs each fetch with the session's access token, refreshed when it is near
its end, and once more after a refresh if FCP answers 401. `Basic` and `None`
stay for servers other than FCP.

---

## 5. JSON schema (v1)

```json
{
  "schema_version": 1,
  "tenant_id":   "acme-corp",
  "etag":        "v=42;t=1700000000",
  "app_name":    "Acme Voice",

  "colors": {
    "primary":     "#1A73E8",
    "accent":      "#34A853",
    "background":  "#FFFFFF",
    "surface":     "#F5F5F5",
    "on_primary":  "#FFFFFF",
    "on_surface":  "#202124",
    "error":       "#D93025"
  },
  "dark_colors": { /* same shape; optional */ },

  "logo":      { "url": "https://cdn.acme.example/logo.svg",
                 "mime": "image/svg+xml",
                 "sha256": "a3f1..." },
  "logo_dark": { "url": "https://cdn.acme.example/logo-dark.svg",
                 "mime": "image/svg+xml",
                 "sha256": "b9c2..." },
  "icon":      { "url": "https://cdn.acme.example/icon.png",
                 "mime": "image/png",
                 "width": 512, "height": 512,
                 "sha256": "ff10..." },

  "ringtones": [
    { "name": "Acme Default",
      "default": true,
      "asset": { "url": "https://cdn.acme.example/ring.ogg",
                 "mime": "audio/ogg",
                 "sha256": "deadbeef..." } }
  ],

  "links": {
    "support": "https://acme.example/support",
    "privacy": "https://acme.example/privacy",
    "terms":   "https://acme.example/terms",
    "website": "https://acme.example"
  },

  "dial_plan": {
    "prefix_strip": ["+1"],
    "short_codes":  { "1000": "sip:frontdesk@acme-corp",
                      "1001": "sip:helpdesk@acme-corp" }
  }
}
```

### Field rules

- **`schema_version`** pinned to `1`. Servers should emit the highest version the
  client understood in its `Accept` header; for now Anvil only understands `1`.
- **`tenant_id`** is opaque. Anvil uses it as the cache key; it is **not** the
  same as the SIP domain (a single domain may host multiple branded tenants).
- **Colors** are `#RRGGBB` or `#RRGGBBAA`. Invalid colors → field ignored, UI
  falls back to its default token for that slot.
- **Assets** are fetched separately over HTTPS and verified by `sha256`.
  Mismatch → asset dropped, the rest of the profile stands. Max size 5 MB each.
- **Missing optional fields** never block rendering — the UI must degrade.

---

## 6. Asset fetch and cache

Assets are referenced by URL + SHA-256. Anvil:

1. Checks the filesystem cache (keyed by sha256). If present, done.
2. Fetches over HTTPS (follows one redirect).
3. Verifies sha256. Mismatch → error, asset treated as absent.
4. Writes to cache, keyed by sha256 (so identical assets across tenants share).
5. Populates `BrandAsset.bytes` in the delivered `BrandProfile`.

Cache eviction: LRU, 50 MB default. Content-addressed storage means assets
never need invalidation — a new logo has a new hash and lives alongside the
old one until evicted.

---

## 7. Refresh

- On login, always.
- On REGISTER refresh (every `register_expires` seconds), opportunistically —
  but only if > 1 hour since the last fetch.
- On explicit `Anvil::refresh_brand()` call from the UI (e.g. a "reload
  branding" debug action).

Server-pushed updates (SIP NOTIFY with event package `fcp-brand`) are a
post-1.0 consideration.

---

## 8. Security considerations

- Provisioning URL must be HTTPS. Anvil rejects `http://` unless the
  host is this machine (`127.0.0.1`, `::1`, `localhost`): a developer's FCP.
  Asset URLs follow the same rule.
- Asset URLs may be any origin; they're fetched with no credentials. Don't
  serve private assets from a URL that depends on auth.
- SVG assets are rendered by the UI framework. Both Flutter's `flutter_svg`
  and web renderers are known to have sandboxing edge cases; prefer PNG for
  logos when the tenant has sensitive brand IP.
- Cached profiles may contain PII-ish strings (e.g. support phone numbers in
  `app_name` for small tenants). The cache should be per-user on multi-user
  desktop OSes, which `dirs::cache_dir()` gives us.
- `sha256` verification is not a signature. A MITM who controls the CDN **and**
  the provisioning endpoint could coordinate a swap. For that threat model,
  add signed profiles in v2 — `signature`, `signer_key_id`, keys pinned in the
  app binary.

---

## 9. Example flow (happy path)

```
Client                                    FCP
  │                                        │
  │  REGISTER sip:alice@acme.example       │
  ├───────────────────────────────────────►│
  │                         200 OK          │
  │           X-FCP-Provisioning-Url: ...   │
  │◄───────────────────────────────────────┤
  │                                        │
  │  GET /brand/v1?aor=sip%3Aalice%40...   │
  │  Authorization: Bearer eyJ...           │
  │  If-None-Match: "v=41;t=169..."         │
  ├───────────────────────────────────────►│
  │                         200 OK          │
  │               ETag: "v=42;t=170..."     │
  │                 (JSON body)             │
  │◄───────────────────────────────────────┤
  │                                        │
  │  GET https://cdn/logo.svg              │
  ├──────────────────► CDN                  │
  │  GET https://cdn/icon.png              │
  ├──────────────────► CDN                  │
  │                                        │
  │  (emit Event::BrandUpdated)            │
```

---

## 9a. What Anvil does (A2)

- **The model reads the wire leniently.** Each color slot is optional; one
  that is not `#RRGGBB`/`#RRGGBBAA` is dropped, and the UI keeps its own
  token. Fields the server leaves out are empty.
- **`HttpBrandProvider`** (`anvil-brand`):
  - `GET {provisioning_url}/brand/v1?aor=…` with `If-None-Match`.
  - 200 gives the profile, its `ETag` kept. 304 keeps what is held.
  - 404 is `BrandFetchOutcome::NoBrand`. 401 and 403 are
    `AnvilError::AuthRejected`.
  - Each asset is fetched (one redirect at most, 5 MB at most) and checked
    against its SHA-256. An asset that fails is dropped and the rest of the
    profile stands.
- **`FsBrandCache`** (`anvil-brand`):
  - A profile per tenant without its bytes, and assets by hash under
    `assets/`.
  - Each asset's `local_path` is set on load.
  - Which tenant an account's brand came from is remembered, so a launch
    can show it before anything is fetched.
  - Assets are evicted least recently used past 50 MB, never one a cached
    profile names.
- **The driver** (`anvil-core`, when `BrandConfig.provider` is set):
  - The cached profile is sent as `Event::BrandUpdated` at start, then a
    fetch runs, concurrently with REGISTER.
  - The provisioning URL is `AccountConfig.provisioning_url`, else learned
    from a REGISTER 200 OK's `X-FCP-Provisioning-Url`.
  - Fetches again on a registration refresh at most hourly, and on
    `Anvil::refresh_brand()`.
  - A 404, or a refused credential, clears what is held and sends
    `Event::BrandCleared`. A refused credential also sends `Event::Error`.
- **`anvil-fcp`**: `brand_config(client, cache_dir)` gives an account signed
  in to FCP its provider and cache. `anvil-cli` uses it and prints
  `[brand] …`.
- **FFI**: `ANVIL_EVENT_KIND_BRAND_UPDATED` with `brand_json` (the profile
  as JSON with `local_path`s and no bytes), and
  `ANVIL_EVENT_KIND_BRAND_CLEARED`.
- **Not yet done:** the `/.well-known/fcp-provisioning` fallback (step 3 of
  §2). A signed-in account always knows its server.

Tested against a running FCP: `crates/anvil-fcp/tests/fcp.rs`
`anvil_takes_on_the_tenants_brand` covers the first fetch with its files,
the cached brand at the next launch with no refetch, a change brought by
`refresh_brand`, and removal.

## 10. Open questions

1. **Auth exchange.** Settled: the app's session (§4).
2. **Multi-account.** A desktop user signed into two accounts (personal +
   work) sees two brands. UI choice: persistent shell branding vs. per-call
   brand swap. Punt until we have concrete UX.
3. **Signed profiles.** Add in v2 of the schema once we know the key
   distribution story.
4. **Localization.** `locale` query param is accepted but the v1 schema has
   no `translations` block. Add when a tenant actually asks for it.
