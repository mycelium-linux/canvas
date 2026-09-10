# Browser engines for Canvas: the decision

*Decided 2026-09-10. Produced by a three-agent research pass — Servo's embedding API read from `servo/servo` main via `gh api`, the WPT numbers computed locally from two `summary_v2.json` files off wpt.fyi, and six weeks of agent browsing measured by parsing `tool_use` blocks out of all 581 transcripts under `~/.claude/projects`. Nothing was compiled and nothing was run: three other workflows owned the machine, and this box has hit OOM and disk exhaustion during parallel builds before. So **no one in this evaluation has loaded a single page in Servo.** Every claim below is read from source, computed from a published test run, or counted from transcripts — and the three places where that leaves inference rather than measurement are marked in place.*

---

## 1. The decision

**Three needs, three different answers, and Servo gets none of them today.**

| Need | Decision | Why in one line |
|---|---|---|
| 1. Per-agent browsing | **Owned Chromium via Playwright `--isolated`** | The capability is already installed, already used in seven FPL scripts, and carries zero web-compat risk. Tenet 10. |
| 2. Web surfaces in Canvas | **Blitz — after the spike that already exists is run once** | For content Canvas authors, no-JS is a feature. But run `spikes/blitz-html` before adopting anything; `out/` is empty. |
| 3. Agent attention display (dock-8cd) | **Neither. It is not a browser problem.** | dock-8cd as filed is file-, PDF- and code-centric. Web pages appear nowhere in it. |
| Servo | **Not yet.** Trigger in §8. | Every monthly release is a breaking bump by stated policy, there is no prebuilt embedder artifact, and its dominant failure mode is silent. |

The reframing in the brief — "an owned Servo instance would serve three needs at once" — does not survive contact with the API. The three needs are not additive; **two of them are in direct tension.** Isolation (need 1) forces one OS process per agent, because Servo cannot host two instances in one process. Process-per-agent then forces the pixels across a *process* boundary for needs 2 and 3, on top of the GL-to-wgpu mismatch that already has no clean answer. Buying one engine to serve all three buys a harder version of two of them.

> **The premise that dissolves needs 2 and 3.** "Canvas composites into a wgpu scene, so a renderer must produce a texture, not own a window" is true, and it is already solved in this tree without any engine. `crates/canvas/src/main.rs` uses `iced::widget::image::Handle::from_rgba` at four sites, and the adopted pattern for a foreign renderer's pixels on the wall is `pdftoppm` — an out-of-process rasterizer producing an RGBA buffer, keyed by sha256 in `media.rs`, uploaded through iced's image pipeline. **Anything that emits `Vec<u8>` RGBA8 is already compositable.** That is not a Servo property, it is not a Blitz property, and it is not a discriminator between them. It also means a captured Chromium viewport is compositable by the same path.

---

## 2. The deciding evidence

Two things decide this, and they point the same way for different reasons.

### 2a. Embedding maturity: the API is good, the *distribution* is not

The encouraging part first, because it is real and it is better than Servo's reputation. `libservo` was renamed `servo` and has been on crates.io since 2026-04-13; 0.5.0 shipped 2026-08-17. The interface is boring and well designed — `ServoBuilder{opts, preferences, event_loop_waker, protocol_registry} -> Servo`, then `WebViewBuilder::new(&servo, rendering_context) -> WebView`, with `ServoDelegate`/`WebViewDelegate` callbacks and a documented five-step embedding guide in `components/servo/lib.rs`. Headless works today: `SoftwareRenderingContext::new(size)` is a public standalone constructor needing no window and no display server, and Servo's own integration tests run on it. And `servo-webdriver-server` 0.5.0 is published, with `start_server(port, embedder_sender, event_loop_waker, default_preferences)` spawning a W3C HTTP server you feed into `Servo::execute_webdriver_command`. You do not build a control surface from scratch. On the *interface* axis, tenet 4 is satisfied on Servo's side.

What disqualifies it is everything around the interface:

- **Every monthly release is a breaking version bump, by policy.** Servo states it outright: "each monthly release will have a breaking version number." Worse for a consumer: "In general, dependencies of servo, like servo-base and servo-script, do not use semver. Any release may include breaking changes." `RenderingContext` — Canvas's actual integration point — lives in `paint_api`, one of the crates with no semver guarantee at all. This is a standing monthly upgrade tax on a load-bearing dependency. Tenet 11 says a new moving part must earn its place; this one bills monthly.
- **There is no prebuilt embedder artifact.** servo.org/download ships the servoshell *application* only. The forward path is `servo-capi` (`ffi/capi/`, cdylib, cbindgen, live CI job), and Servo's own June 2026 post describes it as "they've started designing a wrapper C API." Started designing. **Today, Canvas builds Servo from source or does not use Servo.** That is a 1.99 GB tree and 1,137 crates resolved in `Cargo.lock`, on a machine that has already OOM'd during parallel compiles.
- **docs.rs failed to build `servo` 0.5.0** inside its sandbox limits; 0.4.0 is the last successful docs build. That is an independent data point that this dependency sits at the edge of what ordinary build infrastructure absorbs.
- **Verso is dead.** `versotile-org/verso` was archived 2025-10-08 — "unable to keep pace with these updates due to limited manpower and funding." Any Verso-era write-up making Servo embedding look ergonomic is stale. What remains is Servo's lower-level API plus servoshell as reference.

> **Verified.** Read on `servo/servo` main: `components/servo/{lib.rs,servo.rs,webview.rs,site_data_manager.rs}`, `components/shared/paint/rendering_context.rs`, `components/config/{opts.rs,prefs.rs}`, `components/webdriver_server/lib.rs`, `ports/servoshell/{webdriver.rs,desktop/headless_window.rs}`, `ffi/capi/{Cargo.toml,README.md}`. crates.io version list and dates from the API; `servo-0.5.0.crate` unpacked to confirm the published manifest uses real version deps, not the git deps main uses. `gh api repos/versotile-org/verso` → `archived=true`.
>
> **Unverified, and it is the honest gap.** Nobody compiled it. Cold build wall time on this aarch64 Mac is unknown; the ~13 min figure is Servo's own CI with warm sccache. Whether crates.io `servo` 0.5.0 builds clean on macOS aarch64 at all is unknown — the docs.rs failure is Linux-sandbox evidence and settles nothing here. **And memory per instance is unknown**, which matters most, because an agent fleet is the entire premise. No authoritative benchmark surfaced and I will not invent one. Structurally each instance carries a full SpiderMonkey runtime, WebRender, font and image caches, and N script+layout threads — browser-tab-class, not lightweight — but that is reasoning, not measurement.

### 2b. Multiple instances and state isolation: the answer is no, on both

This is the finding that ends the "one engine, three needs" story, and it is verified from source rather than inferred:

- `pub struct Servo(Rc<ServoInner>)` and `pub struct WebView(Rc<RefCell<WebViewInner>>)` — `Rc`, therefore `!Send`/`!Sync`, pinned to one thread.
- `ServoInner._js_engine_setup` calls `script::init()` for single-process instances, and mozjs's `JSEngine::init()` can only be called once per process. SpiderMonkey cannot be re-initialized after `JS_ShutDown`.
- `static OPTIONS: OnceLock<Opts>` (`components/config/opts.rs:272`) — options are set once per process and immutable after. `static PREFERENCES: RwLock<Preferences>` (`prefs.rs:16`) is process-global and mutable, and `Servo::set_preference` writes to it.
- Servo's own test file states it plainly: "Since all Servo tests must run serially on the same thread."

So the supported model is N WebViews inside ONE Servo instance, and the per-profile knobs that would make that safe do not exist at WebView granularity. `Opts.config_dir` and `Opts.temporary_storage` are real and genuinely threaded into `new_resource_threads`, but they sit behind the process-global `OnceLock` — **one profile per process.** `WebViewBuilder` exposes no profile and no private option; its knobs are delegate, url, hidpi_scale_factor, user_content_manager, clipboard_delegate, gamepad_delegate. The constellation carries `public_resource_threads`/`private_resource_threads`, but top-level webviews are created with a hardcoded `let is_private = false;` at `constellation.rs:3070` and `:3271` — private is only ever inherited from a parent browsing context. `SiteDataManager` gives you `clear_site_data`/`clear_cookies` — cleanup and inspection, not partitioning.

**Servo does not give you cheap per-agent contexts. It gives you expensive per-agent processes.**

### 2c. Web compatibility: solid numbers, and they are not the reassuring kind

Measured against the *identical* WPT revision `1fad8a9882`, linux-22.04, both runs dated 2026-09-09, scored locally rather than quoted:

| | Chrome 153 stable | Servo 0.6.0 nightly |
|---|---|---|
| Overall WPT | **92.1%** (n=123,106) | **74.0%** (n=122,648) |
| Servo's own pinned corpus | — | 66.3% (57,716 tests, run 2026-09-10) |

The trend is the number most likely to be missing from anyone's priors, and it deserves stating fairly: 26.5% (Apr 2023) → 49.3% (Oct 2025) → 59.2% (Apr 2026) → 66.3% (Sep 2026). That is roughly **17 points in eleven months**. Any judgement of Servo older than about six months is stale, and this decision is a quarterly re-check, not a permanent no.

But the gap is concentrated exactly where the operational web lives (Servo % / Chrome %):

| Area | Servo | Chrome | What depends on it |
|---|---|---|---|
| `/accname/` | **0.0** | 86.7 | Every AX-snapshot driving pattern agents use today |
| `/webdriver/tests/bidi/` | 0.0 | 83.0 | No BiDi, and no CDP either |
| `/service-workers/` | 2.3 | 86.4 | SPA shells |
| `/webauthn/` | 3.0 | 98.1 | Passkey login — simply unavailable |
| `/svg/` | 23.5 | 88.3 | Grafana, D3, Recharts, every dashboard |
| `/IndexedDB/` | 39.6 | 100.0 | MSAL, Firebase, Amplify token caches |
| `/webdriver/tests/classic/` | 75.3 | 92.5 | The control surface itself |

And the configuration you would actually embed is **worse than the configuration those numbers were measured under.** Read from `components/config/prefs.rs`, default-OFF: IndexedDB, service workers, IntersectionObserver, WebGL2, WebGPU, WebRTC, Web Animations, adoptedStyleSheets, OffscreenCanvas, Credential Management, and — critically — `accessibility_enabled: false`. IntersectionObserver off breaks lazy-loading and infinite scroll everywhere. adoptedStyleSheets off breaks Lit and much of the web-components ecosystem. Embedding Servo means owning a pref matrix, and benchmarking against the published 66–74% without constructing that matrix will produce a worse result than expected for reasons that are invisible.

> **Verified.** Both `summary_v2.json` files downloaded (~13 MB each) and scored with a local script; every percentage above is computed here, not quoted. Pref defaults read from `prefs.rs` directly, from the `const_default()` block, not from documentation.
>
> **Inferred, not verified.** No site-level claim. GitHub, AWS/GCP consoles, Grafana and vendor portals were *not* tested. Predictions about them are dependency reasoning — custom elements + adoptedStyleSheets + IntersectionObserver for GitHub; IndexedDB + service workers + SVG + WebAuthn for consoles — and should be treated as hypotheses a 30-minute servoshell session would settle.

---

## 3. The failure mode: this is what actually decides it

The brief asks whether an agent could tell a page rendered wrongly. **Mostly no, and the shape of the "no" is specific enough to state exactly.**

Partition the 109,216 tests Chrome fully passes:

| Layer | Servo fully passes | Servo scores **zero** | Fails *loudly* (error/timeout/crash) |
|---|---|---|---|
| testharness (DOM / API semantics) | 83.6% | **11.4%** | 1.6% |
| reftest (pixel rendering) | 66.4% | 31.9% | 1.6% |

All 9,481 of those total-failing harness tests carry WPT status `O` — harness OK. **No crash, no timeout, no error status. The page loaded, the script ran to completion, and every assertion was wrong.** Only 1.6% fail in a way any error handler would catch.

The mechanism is visible in the IDL numbers: Servo exposes 23,037 of 36,584 IDL members (63.0%) against Chrome's 93.4%. A third of the web API surface is absent or wrong, and the dominant runtime manifestation is a `TypeError` thrown inside page JS that halts one script and leaves a **partially-built DOM**. Not a blank page. Not an error page. A page that is merely *shorter than it should be*.

> **This is `docs/widget-quality.md` rule 1 relocated to the agent layer.** An agent reading that DOM has no signal distinguishing "this dashboard shows no alerts" from "the fetch that populates alerts threw." It will then report the first. That is synthetic data rendered as real data, one level up from the widget — and it is worse than the `mock_price()` violation, because at least `mock_price()` is deterministic and lives at a known line number. Rule 3 says absent data must state why. Servo cannot say why, because it does not know anything is absent.

Two consequences that close off the obvious mitigations:

- **Transport is not the risk, which removes the cheapest detector.** `components/net/Cargo.toml` on main: hyper with http1+http2, hyper-rustls, rustls with aws-lc-rs and `rustls-platform-verifier`, async-compression with brotli/gzip/zlib/zstd. Modern TLS against the platform store, HTTP/2, every content-encoding a site will negotiate. **Sites will not fail to load in Servo. They will fail to work.** Connection failure never fires.
- **The console channel does not carry what you need.** `components/shared/embedder/lib.rs` defines `ShowConsoleApiMessage` as "a DOM Console API message" — only what page JS explicitly logs. There is no first-class uncaught-exception callback. A detector built on `notify_crashed` plus console output catches the loud 1.6% and passes the other 98.4% through.

A detector that *would* work exists — an injected `window.onerror`/`unhandledrejection` probe plus a per-target content-plausibility assertion (innerText length, interactive element count, a caller-supplied expected selector). But it is **per-target work**, which erodes the entire "Servo for the common case, Chrome for the hard case" cost argument. And the naive alternative is worse: a screenshot-diff-against-Chrome detector would fire on nearly everything, because pixel parity is 66.4%. You would pay for two engines and use one.

**The finding that inverts the brief's premise:** need 3 — the human seeing what the agent sees — is the need Servo serves *worst*. At two-in-three reftest parity, the human watches a visibly wrong rendering of what the agent is reasoning over. Their check flags defects that are artifacts of the display path, and misses the silent-DOM errors entirely, because those render as plausible-looking shorter pages. "Your eyes are visible to me" is only trustworthy if the picture matches the DOM the agent actually read.

And the AX angle makes it concrete: `/accname/` scores 0.0% and `accessibility_enabled` defaults to false. Accessible-name computation is the substrate under Playwright snapshots, browsermcp's `browser_snapshot`, and claude-in-chrome's `read_page`. **Driving Servo the way agents drive Chrome today walks straight into the engine's single weakest layer.** Servo does expose `WebViewDelegate::notify_accessibility_tree_update` handing you an `accesskit::TreeUpdate` — genuinely attractive on paper — but the tree only became "an actual tree structure" in 0.5.0 (August 2026). Porting existing agent patterns to Servo means switching to raw DOM/innerText reading. That is a change to how the agent tooling works, not a config swap.

---

## 4. What per-agent browsing would buy, against what shared Chrome costs

**It buys one real thing, and the contention argument is weak enough that it should be dropped.**

Measured over 581 transcripts, six weeks, 28 Jul – 8 Sep 2026:

| | Calls | Sessions |
|---|---|---|
| `browsermcp` | **0** | 0 |
| claude-in-chrome | 302 | 10 |
| WebFetch | 452 | 40 |
| WebSearch | 280 | 37 |

Zero. Not "rare" — browsermcp has never been called, ever, while **nine `fpl-desktop-bridge connect --connector browsermcp` processes** from nine distinct ttys are running right now, all fanning into one mcp-proxy and one node process. That is textbook tenet-11 sprawl, and it is a separate cleanup from this decision.

Three claims from the brief that the evidence contradicts:

1. **"Multiple agents contending for it."** There is not one instance of two independent agents driving a browser simultaneously. Three transcript pairs appeared to overlap; all three are artifacts — one is a subagent file nested under its parent with identical 28/28 calls, two are fork/resumes (one pair shares 85 of 144 `toolu_` ids and starts at the same millisecond).
2. **"One shared browser."** claude-in-chrome already isolates at the tab level. The schemas describe a dedicated MCP tab group: `tabs_create_mcp` "creates a new empty tab in the MCP tab group", navigate acts on "the first tab in the session's group", `javascript_tool`'s tabId "must be a tab in the current group." Tab contention, navigation races and focus stealing are mostly already solved.
3. **"Agents browsing the web."** Of 59 navigations with a URL: 40 to `127.0.0.1`, 10 to `100.108.9.93` (the bridge's own listen address), 5 to `localhost`, 2 to a local `file://`, 2 to a public site. **57 of 59 are local.** Agents are screenshotting their own dev servers. Real web access is stateless WebFetch/WebSearch, which parallelizes perfectly and has no browser to contend over.

What per-agent browsing genuinely buys is **the cookie jar**. The MCP tab group lives in the human's Brave `Default` profile — Cookies 1.0 MB, written today, alongside a `Login Data` store. An agent navigating anywhere the human is logged in arrives already authenticated as them and can read the page. The telling detail: this session's permission classifier *blocked* an agent from copying that cookie DB for analysis, while the browser tools hand an agent live use of it with no equivalent check. Exposure is near zero today only because 57 of 59 navigations were local — that is one behavior change away from mattering.

> **And Servo does not fix that. Playwright does, and it is already installed and already adopted.** Every FPL Playwright call site launches a fresh isolated headless Chromium and never touches the user's browser: `coherence/scripts/verify-workbench-browser.mjs`, `coherence/scripts/prove-time-series.mjs`, `fpl-auth/tests/{companies,design,google,billing}-browser.mjs`, `jarvis/tests/enrollment_browser.mjs`, `fab/scripts/test-console.mjs`. `~/Library/Caches/ms-playwright` holds `chromium-1187` and `chromium_headless_shell-1228`. Playwright MCP ships `--isolated` for exactly this. **A per-agent browsing lane is a config change, not an engine.** Tenets 10 and 11, both.

The ecosystem agrees, for what that is worth: every published answer to "one browser, N agents" is a multiplexer or a session parameter — multiplexed reverse proxies with per-tab lock-conflict errors, OpenTabs sessions per MCP client, Cloudflare Browser Run, `microsoft/playwright#40585` proposing a `sessionId` for multi-session isolation. Nobody is writing a browser to solve this.

> **The one thing that could weaken §4, and it is worth checking before citing this.** Whether the claude-in-chrome MCP tab group is scoped per *session* or per *machine* rests entirely on schema text ("the session's group"), not on observed behavior — the classifier blocked reading tab-list payloads out of transcripts. If groups turn out to be per-machine, "tab contention is already solved" weakens considerably. This is the single highest-value unknown in the whole evaluation and it costs one experiment.

---

## 5. The isolation angle

**Correction first, because the brief attributes the requirement to the wrong bead.** `dock-ybc.2` is *briefing surfaces* — the newcomer intro and the recurring executive update. Its relevant clauses are the identity gate (via `dock-8nz`) and an explicit pointer at `docs/widget-quality.md` rule 1: "a briefing that renders a metric with no source behind it is the fabrication rule at presentation scale." The no-leak requirement lives in **`dock-ybc.11`**, and it names agent attention directly: "agent transcripts and attention (dock-8cd) — what an agent read is often the most revealing thing."

Read against `dock-ybc.11`'s actual design rules, an owned browser instance is **orthogonal at the layer that matters, and mildly negative at the layer above it.**

- `dock-ybc.11` requires *assembly per scope rather than filtering after*, and fail-closed on unresolved identity. Per-agent browser state is one input to a scope, not the scope itself. Giving each agent its own cookie jar does not assemble anything per scope; it prevents one specific cross-contamination — the human's session bleeding into an agent's view. That is worth having, and `playwright --isolated` delivers it today.
- Servo delivers it *worse*. Because `OPTIONS` is a process-global `OnceLock` and `PREFERENCES` a process-global `RwLock`, Servo's isolation granularity is the OS process, not the WebView. You get the same isolation Chromium already gives you, at the cost of a monthly-breaking 1,137-crate dependency and one SpiderMonkey runtime per agent whose memory footprint nobody has measured.
- **Need 3 makes isolation strictly harder, not easier.** Routing an agent's rendered page onto the wall means painting the human's authenticated sessions on a shared surface. `dock-8cd` already requires `dock-7pa` presence gating with "if it is not the owner watching, redact" — and `dock-7pa`'s own rule is that Unknown/Unavailable must fail closed to blurred. A web-page texture inherits that requirement and raises the stakes on it. It is not a free addition to an existing surface.

**The isolation argument does not carry Servo.** It is real, it is already satisfiable, and the version of it that involves a browser texture on a wall is a new privacy obligation rather than a benefit.

---

## 6. First move

Two experiments. Neither touches the repository, neither adds a dependency, and each is falsifiable in both directions.

### 6a. The one the brief asked for: one agent, one real page, owned instance vs Chrome

Pick a page the agents *actually need* and that is not localhost — the honest candidate set is small, because 57 of 59 navigations were local. Take one real page with a login behind it (a vendor portal, a dashboard). Then:

1. Drive it through a fresh `playwright --isolated` headless Chromium: navigate, capture `document.body.innerText`, capture the accessibility snapshot, capture a screenshot.
2. Drive the same URL through claude-in-chrome in the human's Brave profile.
3. Diff the three artifacts.

**What it proves.** If the isolated instance reaches the page and produces a usable snapshot, the per-agent lane is done — need 1 is closed with a config change and Servo loses its strongest leg. If the isolated instance *cannot* reach the page because the auth lives in the human's profile, that is the actual product question surfacing: agent browsing that requires the human's identity is not isolatable by any engine, Servo included, and the answer is a service credential, not a browser.

**Cost:** an afternoon, no build, no new dependency.

### 6b. The one that would settle Servo, deliberately off the critical path

Run `servoshell --headless` against that same page. Capture `document.body.innerText` and a screenshot; diff both against the Chromium arm. Then open `about:memory` — or call `Servo::create_memory_report` — with 1, 4, and 8 tabs and read the reporter.

That single session answers the three things this evaluation could not: does a real page the agents care about actually work; how visibly wrong is the render; and **what does one instance cost in RSS**, which is the largest unknown here and the one that decides whether "an instance per agent" is a sentence that means anything.

> Queue any build behind the `lockout` skill. This machine has OOM'd and filled disk during parallel compiles, and a 1.99 GB tree with 1,137 crates is not the thing to find that out with again.

### 6c. And the free one, which should happen first

**Run the Blitz spike that already exists.** `spikes/blitz-html/src/main.rs` is a real ~150-line program with a `shot` path that parses HTML through blitz-html/blitz-dom, paints into an anyrender scene, and rasterises to RGBA8 via `vello_cpu` with no window ever created — exactly the `Vec<u8>` that `Handle::from_rgba` consumes. `out/` is empty; it has never been run.

> **Correction to an earlier research note.** One pass reported this spike's `main.rs` as a one-line stub. It is not. Verified by reading the file: usage string, `tera` and `shot` subcommands, timing instrumentation, PNG output. The spike is finished and unexercised.

One command against `fixtures/css-probe.html` answers need 2 for the cost of running a binary somebody already wrote. If the buffer comes back correct and fast enough, need 2 is closed against a beta-quality dependency with a 251k-download DOM crate, no JS engine, and no monthly breaking bump. If it is too slow or the CSS is wrong, that is a real finding and it costs nothing to learn.

---

## 7. What we would not do

- **Do not adopt Servo as a Canvas dependency in v0.** 1.99 GB, 1,137 crates, monthly breaking releases by stated policy, sibling crates with no semver at all, and the integration point (`RenderingContext`, in `paint_api`) sitting on the weakest-guaranteed crate in the set. That is a recurring maintenance obligation, not a one-time cost, on a capability the ecosystem already owns. Tenet 11.
- **Do not build the IOSurface zero-copy path.** surfman's macOS backend is CGL + IOSurface, IOSurface is Metal-importable, and in principle a handle could be smuggled to wgpu. In practice `OffscreenRenderingContext`'s `Framebuffer.texture_id` and `framebuffer_id` have no public accessor — grepped, zero hits — and `OffscreenRenderingContext::new` is private, reachable only through `WindowRenderingContext::offscreen_context()`, so it presupposes a window it was supposed to eliminate. Reaching for the handle anyway means coupling Canvas to surfman internals: a direct tenet-2 violation, bespoke and unsupported, to optimise a path §1 shows we do not need.
- **Do not plan against `servo-capi`.** "Started designing." A stable ABI would change this decision materially. It is not a schedule input.
- **Do not treat anything written about Verso as current.** Archived 2025-10-08. The ergonomic high-level embedding layer that made Servo look approachable does not exist.
- **Do not build a Servo-vs-Chrome screenshot-diff fallback detector.** 66.4% pixel parity means it fires on nearly everything.
- **Do not port the AX-snapshot driving pattern onto Servo.** `/accname/` 0.0%, `accessibility_enabled` false by default. It is the least-implemented layer in the engine and the one every existing agent driver reasons over.
- **Do not adopt Blitz into `crates/canvas` before §6c has been run once.** It is `0.3.0-beta.2`. The spike is detached from the workspace on purpose; keep it that way until there is an output file to look at.
- **Do not route any agent's web view onto the wall before `dock-7pa` presence gating exists.** `dock-8cd` requires redaction for a non-owner viewer on *files*. A browser viewport carrying the human's authenticated sessions is a strictly larger obligation, and `dock-7pa`'s fail-closed rule has to be real code before this becomes a surface.
- **Do not fold the browsermcp cleanup into this decision.** Nine processes, zero calls in their lifetime, and a human "click Connect in the toolbar" gate that Servo does not fix and deletion does — but confirm nothing outside `~/.claude/projects` depends on it first. Codex has its own native messaging host registered and its transcripts are not in that corpus. Separate bead, separate verification.
- **Do not start any of this on `rename/dock-to-canvas`.** It carries uncommitted `canvas-metrics` work and a build repaired two commits ago.

---

## 8. The trigger to revisit

Servo is improving at roughly 1.5 WPT points a month. Deciding "no, permanently" would be as wrong as deciding "yes, now" — this is a quarterly re-check against `wpt.servo.org/scores.json`, with the thresholds fixed in advance so the re-check is a measurement and not an argument:

| Gate | Threshold | Why this number |
|---|---|---|
| Agent browsing viable | Silent-failure fraction — harness tests Chrome passes that Servo fails with status `O` — **below 3%** (today: 11.4%) | This, not the headline score, is what makes an agent report confident nonsense |
| Attention display viable | Reftest parity **above 90%** (today: 66.4%) | Below that the human is checking the renderer, not the agent |
| AX driving viable | `/accname/` **above 80%** (today: 0.0%) | Below that, every existing driver pattern has to be rewritten |
| Adoptable as a dependency | `servo-capi` ships a stable ABI **or** the 0.1.x LTS line carries non-breaking releases for two consecutive quarters | Removes the from-source build and the monthly upgrade tax |

**The one argument that would flip this early:** if agent browsing is scoped to a known, bounded set of origins — your own docs, rustdoc, one vendor's API reference — the compat risk collapses to something you measure once and allowlist, and Servo at 83.6% DOM parity on static-ish content is fine for that today. So the question to put to whoever owns `dock-8cd` is not "is Servo web-compatible." It is **"is the agent's browsing surface bounded?"** If it is bounded, Servo becomes viable sooner than these gates suggest. If it is "whatever the agent decides to look at," Servo is worse than no browser at all — because a wrong render the agent cannot detect produces a confident wrong report, and a tool that refuses is strictly better than a tool that lies.

That is a product decision. No further engine research settles it.