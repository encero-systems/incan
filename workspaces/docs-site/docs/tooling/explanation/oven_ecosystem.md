---
title: The Oven ecosystem
hide:
  - toc
---

<!-- markdownlint-disable MD033 MD060 -->

# The Oven ecosystem

One authored project flows top to bottom through this map: what the author writes, how Oven resolves and plans it, how the compiler service and the direct `rustc` executor bake it into Loaves the store keeps, and how the registry moves those Loaves between machines. Border style carries RFC status. The two gold dotted nodes, the store's unit identity and `incan.pub`, are the two designs currently in review; the solid gold node is the Loaf itself, the unit everything else bakes, stores, reuses, and exchanges.

This page is a map, not a specification. Each box names the RFC that owns it, and the table at the end collects them. Where the map and an RFC disagree, the RFC is right and the map has a defect.

<figure class="inc-oven-map">
<div class="inc-oven-map__scroll">
<svg class="inc-oven-map__svg" viewBox="0 0 1320 1500" xmlns="http://www.w3.org/2000/svg" role="img" aria-labelledby="oven-map-title oven-map-desc">
      <title id="oven-map-title">The Oven Ecosystem</title>
      <desc id="oven-map-desc">A five-tier architecture map. An authored project — two language facets, a lifecycle layer and one manifest — feeds Oven's resolver and unit graph, which draw on SDK components and an optional Cargo compatibility mode. A plan of units and identities reaches the compiler service, which exposes the Incan facet provider API back up to the unit graph and emits Rust to a direct rustc executor; that executor compiles units and seals each as a Loaf — payload, plan, identity and receipt in one immutable artifact — which a content-addressed store keeps and hands back to any later plan with the same identity. The store materialises linked outputs, and publishes the source Loaf with attested assets to the incan.pub registry, which admits assets back only on exact unit identity, projects an artifact graph, and is copied by mirrors, while crates.io is consumed as source only. Two command surfaces, incan and oven, sit beneath, with delegation running one way from incan down to oven.</desc>
      <defs>
        <marker id="oven-map-arrow" markerWidth="8" markerHeight="6" refX="7" refY="3" orient="auto"><polygon points="0 0, 8 3, 0 6" fill="#c1c8d0"/></marker>
        <marker id="oven-map-arrow-link" markerWidth="8" markerHeight="6" refX="7" refY="3" orient="auto"><polygon points="0 0, 8 3, 0 6" fill="#48f0ef"/></marker>
      </defs>
      <rect width="100%" height="100%" fill="#030304"/>
      <!-- ================= ZONES ================= -->
      <rect class="zone" x="40" y="80" width="1200" height="180" rx="8"/>
      <rect class="mask" x="52" y="84" width="232" height="12" rx="2"/>
      <text class="zn" x="56" y="93">PROJECT · WHAT THE AUTHOR WRITES</text>
      <rect class="zone" x="40" y="300" width="1200" height="280" rx="8"/>
      <rect class="mask" x="52" y="304" width="212" height="12" rx="2"/>
      <text class="zn" x="56" y="313">OVEN · RESOLVE AND PLAN</text>
      <rect class="zone" x="40" y="620" width="1200" height="276" rx="8"/>
      <rect class="mask" x="52" y="624" width="184" height="12" rx="2"/>
      <text class="zn" x="56" y="633">COMPILE AND STORE</text>
      <rect class="zone" x="40" y="920" width="1200" height="292" rx="8"/>
      <rect class="mask" x="52" y="924" width="200" height="12" rx="2"/>
      <text class="zn" x="56" y="933">REGISTRY AND SOURCES</text>
      <rect class="lane" x="40" y="1252" width="1200" height="140" rx="8"/>
      <rect class="mask" x="52" y="1256" width="196" height="12" rx="2"/>
      <text class="zn" x="56" y="1265">COMMAND SURFACES · RFC 118</text>
      <!-- ================= ARROWS ================= -->
      <!-- feedback channel: artifact graph -> project lifecycle (RFC 079 -> 076) -->
      <path class="e dash" d="M 64,1014 H 32 Q 24,1014 24,1006 V 64 Q 24,56 32,56 H 1100 Q 1108,56 1108,64 V 112"/>
      <rect class="mask" x="500" y="34" width="76" height="12" rx="2"/>
      <text class="al" x="538" y="43" text-anchor="middle">ADVISORIES</text>
      <!-- N4 lifecycle -> N3 manifest -->
      <line class="e" x1="1000" y1="174" x2="928" y2="174"/>
      <rect class="mask" x="938" y="152" width="52" height="12" rx="2"/>
      <text class="al" x="942" y="161">MUTATES</text>
      <!-- N3 manifest -> N6 resolver -->
      <line class="e" x1="740" y1="236" x2="740" y2="332"/>
      <rect class="mask" x="750" y="278" width="60" height="12" rx="2"/>
      <text class="al" x="754" y="287">RESOLVES</text>
      <!-- N6 resolver -> N3 manifest (writes the lock) -->
      <line class="e" x1="844" y1="332" x2="844" y2="236"/>
      <rect class="mask" x="854" y="278" width="76" height="12" rx="2"/>
      <text class="al" x="858" y="287">WRITES LOCK</text>
      <!-- N1 Incan facet -> N5 unit graph -->
      <line class="e" x1="188" y1="236" x2="188" y2="332"/>
      <rect class="mask" x="198" y="278" width="80" height="12" rx="2"/>
      <text class="al" x="202" y="287">SELECTS UNITS</text>
      <!-- N2 Rust facet -> N5 unit graph (shares the label above) -->
      <line class="e" x1="468" y1="236" x2="468" y2="332"/>
      <!-- N6 resolver -> N5 unit graph -->
      <line class="e" x1="672" y1="394" x2="576" y2="394"/>
      <rect class="mask" x="586" y="372" width="76" height="12" rx="2"/>
      <text class="al" x="590" y="381">RESOLVED</text>
      <!-- N7 SDK components -> N5 unit graph -->
      <line class="e" x1="240" y1="480" x2="240" y2="456"/>
      <rect class="mask" x="250" y="462" width="120" height="12" rx="2"/>
      <text class="al" x="254" y="471">SELECTED COMPONENTS</text>
      <!-- N5 unit graph -> N9 compiler service (the plan: units + identities, RFC 124) -->
      <path class="e" d="M 560,456 V 608 Q 560,616 552,616 H 288 Q 280,616 280,624 V 652"/>
      <rect class="mask" x="384" y="578" width="76" height="12" rx="2"/>
      <text class="al" x="388" y="587">PLAN: UNITS</text>
      <rect class="mask" x="384" y="594" width="76" height="12" rx="2"/>
      <text class="al" x="388" y="603">+ IDENTITIES</text>
      <!-- N9 compiler service -> N5 unit graph: the Incan facet provider API.
           RFC 117 seam - Oven core depends only on this contract, never on the
           compiler service directly. Hops the plan edge at (520,616). -->
      <path class="e" d="M 344,652 V 644 Q 344,636 352,636 H 512 Q 520,636 520,628 V 624 a 8,8 0 0,1 0,-16 V 456"/>
      <rect class="mask" x="530" y="540" width="76" height="12" rx="2"/>
      <text class="al" x="534" y="549">PROVIDER API</text>
      <!-- N8 Cargo compat -> N10 rustc executor (explicit legacy path) -->
      <path class="e dash" d="M 800,556 V 624 Q 800,632 792,632 H 608 Q 600,632 600,640 V 652"/>
      <rect class="mask" x="680" y="612" width="76" height="12" rx="2"/>
      <text class="al" x="684" y="621">LEGACY PATH</text>
      <!-- N9 compiler service -> N10 rustc executor (the compiler service only emits Rust) -->
      <line class="e" x1="368" y1="714" x2="432" y2="714"/>
      <rect class="mask" x="372" y="692" width="56" height="12" rx="2"/>
      <text class="al" x="400" y="701" text-anchor="middle">EMITS RUST</text>
      <!-- N10 rustc executor -> N10b Loaf (compiles the unit and seals it) -->
      <line class="e" x1="632" y1="684" x2="680" y2="684"/>
      <rect class="mask" x="636" y="662" width="40" height="12" rx="2"/>
      <text class="al" x="656" y="671" text-anchor="middle">SEALS</text>
      <!-- N10b Loaf -> N11 store (inserted under its unit identity) -->
      <line class="e" x1="840" y1="684" x2="888" y2="684"/>
      <rect class="mask" x="840" y="662" width="48" height="12" rx="2"/>
      <text class="al" x="864" y="671" text-anchor="middle">STORED</text>
      <!-- N11 store -> N10 rustc executor (the executor decides reuse) -->
      <path class="e" d="M 904,776 V 780 Q 904,788 896,788 H 624 Q 616,788 616,780 V 776"/>
      <rect class="mask" x="846" y="794" width="40" height="12" rx="2"/>
      <text class="al" x="866" y="803" text-anchor="middle">REUSES</text>
      <!-- N11 store -> N12 outputs -->
      <path class="e" d="M 920,776 V 828 Q 920,836 912,836 H 840"/>
      <rect class="mask" x="842" y="818" width="72" height="12" rx="2"/>
      <text class="al" x="878" y="827" text-anchor="middle">MATERIALISES</text>
      <!-- N10b Loaf -> N14 incan.pub: publication ships the source Loaf plus its attested
       baked assets - never the linked outputs. -->
      <path class="e" d="M 680,760 H 672 Q 664,760 664,768 V 780 a 8,8 0 0,0 0,16 V 936 Q 664,944 672,944 H 892 Q 900,944 900,952"/>
      <rect class="mask" x="600" y="854" width="56" height="12" rx="2"/>
      <text class="al" x="656" y="863" text-anchor="end">PUBLISH</text>
      <rect class="mask" x="572" y="870" width="84" height="12" rx="2"/>
      <text class="al" x="656" y="879" text-anchor="end">LOAF + ASSETS</text>
      <!-- N14 incan.pub -> N11 store (admission is by unit identity, RFC 125) -->
      <line class="e" x1="1000" y1="952" x2="1000" y2="776"/>
      <rect class="mask" x="1010" y="854" width="84" height="12" rx="2"/>
      <text class="al" x="1014" y="863">IMPORTS ASSETS</text>
      <rect class="mask" x="1010" y="870" width="96" height="12" rx="2"/>
      <text class="al" x="1014" y="879">BY UNIT IDENTITY</text>
      <!-- N14 incan.pub -> N13 artifact graph -->
      <line class="e" x1="784" y1="1028" x2="504" y2="1028"/>
      <rect class="mask" x="608" y="1006" width="60" height="12" rx="2"/>
      <text class="al" x="612" y="1015">PROJECTS</text>
      <!-- N14 incan.pub -> N15 mirrors -->
      <path class="e dash" d="M 820,1076 V 1088 Q 820,1096 812,1096 H 288 Q 280,1096 280,1104 V 1116"/>
      <rect class="mask" x="600" y="1074" width="72" height="12" rx="2"/>
      <text class="al" x="604" y="1083">STATIC COPY</text>
      <!-- N16 crates.io -> N14 incan.pub (registry-built assets via external-source record) -->
      <line class="e dash" x1="1060" y1="1116" x2="1060" y2="1076"/>
      <rect class="mask" x="1070" y="1090" width="92" height="12" rx="2"/>
      <text class="al" x="1074" y="1099">EXT-SOURCE REC</text>
      <!-- N14 incan.pub -> N6 resolver (fetches index, manifests, signed source Loaves) -->
      <path class="e" d="M 1216,1040 H 1248 Q 1256,1040 1256,1032 V 388 Q 1256,380 1248,380 H 1216"/>
      <rect class="mask" x="1226" y="358" width="88" height="12" rx="2"/>
      <text class="al" x="1230" y="367">INDEX + LOAVES</text>
      <!-- N16 crates.io -> N6 resolver (external, source archives only; hops the channel above) -->
      <path class="e ext" d="M 1216,1152 H 1280 Q 1288,1152 1288,1144 V 428 Q 1288,420 1280,420 H 1264 a 8,8 0 0,0 -16,0 H 1216"/>
      <!-- N17 incan -> N18 oven -->
      <line class="e" x1="608" y1="1326" x2="680" y2="1326"/>
      <rect class="mask" x="614" y="1304" width="60" height="12" rx="2"/>
      <text class="al" x="644" y="1313" text-anchor="middle">DELEGATES</text>
      <!-- ================= NODES ================= -->
      <!-- ---- Zone 1: Project ---- -->
      <rect class="mask" x="64" y="112" width="248" height="124" rx="6"/>
      <rect class="box" x="64" y="112" width="248" height="124" rx="6"/>
      <text class="nm"  x="80" y="142">Incan facet</text>
      <text class="sub" x="80" y="162">.incn sources</text>
      <text class="sub" x="80" y="178">std.registry descriptors</text>
      <text class="rfc" x="80" y="216">RFC 077 · 113</text>
      <rect class="mask" x="344" y="112" width="248" height="124" rx="6"/>
      <rect class="box planned" x="344" y="112" width="248" height="124" rx="6"/>
      <text class="nm"  x="360" y="142">Rust facet</text>
      <text class="sub" x="360" y="162">conventional src/</text>
      <text class="sub" x="360" y="178">[rust.*] for deviations only</text>
      <text class="rfc" x="360" y="216">RFC 119 · LEGACY CRATE DEPS RFC 013</text>
      <rect class="mask" x="656" y="112" width="272" height="124" rx="6"/>
      <rect class="box planned" x="656" y="112" width="272" height="124" rx="6"/>
      <text class="nmm" x="672" y="142">loaf.toml · oven.lock</text>
      <text class="sub" x="672" y="162">one authored manifest</text>
      <text class="sub" x="672" y="178">typed deps: loaf, crate, path</text>
      <text class="sub" x="672" y="194">registry identity, endpoint, trust</text>
      <text class="rfc" x="672" y="216">RFC 117 · LOCK RFC 020</text>
      <rect class="mask" x="1000" y="112" width="216" height="124" rx="6"/>
      <rect class="box draft" x="1000" y="112" width="216" height="124" rx="6"/>
      <text class="nm"  x="1016" y="142">Project lifecycle</text>
      <text class="sub" x="1016" y="162">templates · starters · mixes</text>
      <text class="sub" x="1016" y="178">mutation policy · actions</text>
      <text class="sub" x="1016" y="194">env matrices · receiver-owned</text>
      <text class="rfc" x="1016" y="216">RFC 073 · 074 · 075 · 076 · 078</text>
      <!-- ---- Zone 2: Resolve and plan ---- -->
      <rect class="mask" x="64" y="332" width="512" height="124" rx="6"/>
      <rect class="box planned" x="64" y="332" width="512" height="124" rx="6"/>
      <text class="nm"  x="80" y="362">Unit graph and host providers</text>
      <text class="sub" x="80" y="382">host and target domains; a role per unit</text>
      <text class="sub" x="80" y="398">build scripts are admitted and receipted; proc-macros are host units</text>
      <text class="sub" x="80" y="414">native linkage, carriers, cross compilation</text>
      <text class="rfc" x="80" y="436">RFC 119</text>
      <rect class="mask" x="672" y="332" width="544" height="124" rx="6"/>
      <rect class="box planned" x="672" y="332" width="544" height="124" rx="6"/>
      <text class="nm"  x="688" y="362">Resolver</text>
      <text class="sub" x="688" y="382">registry indexes, version and toolchain requirements</text>
      <text class="sub" x="688" y="398">never executes package code</text>
      <text class="rfc" x="688" y="436">RFC 117 · OFFLINE AND LOCKED RFC 020</text>
      <rect class="mask" x="64" y="480" width="440" height="76" rx="6"/>
      <rect class="box" x="64" y="480" width="440" height="76" rx="6"/>
      <text class="nm"  x="80" y="508">SDK components and compiled providers</text>
      <text class="sub" x="80" y="526">stdlib as components, package features</text>
      <text class="rfc" x="80" y="546">RFC 114</text>
      <rect class="mask" x="776" y="480" width="440" height="76" rx="6"/>
      <rect class="box planned ext" x="776" y="480" width="440" height="76" rx="6"/>
      <text class="nm"  x="792" y="508">Cargo compatibility mode</text>
      <text class="sub-x" x="792" y="526">explicit adoption only, never silent authority</text>
      <text class="rfc-x" x="792" y="546">RFC 119</text>
      <!-- ---- Zone 3: Compile and store ---- -->
      <rect class="mask" x="64" y="652" width="304" height="124" rx="6"/>
      <rect class="box" x="64" y="652" width="304" height="124" rx="6"/>
      <text class="nm"  x="80" y="682">Compiler service (Incan facet)</text>
      <text class="sub" x="80" y="702">checked analysis; canonical symbol identity</text>
      <text class="sub" x="80" y="718">executable representation of exports</text>
      <text class="sub" x="80" y="734">interop: Rust-hosted caller, typed C ABI</text>
      <text class="rfc" x="80" y="756">RFC 120 · 106 · 123 · 097 · 116 · 121</text>
      <rect class="mask" x="432" y="652" width="200" height="124" rx="6"/>
      <rect class="box planned" x="432" y="652" width="200" height="124" rx="6"/>
      <text class="nm"  x="448" y="682">Direct rustc executor</text>
      <text class="sub" x="448" y="702">compiles units; seals Loaves</text>
      <text class="sub" x="448" y="718">decides reuse by identity</text>
      <text class="sub" x="448" y="734">Oven Alpha today</text>
      <text class="rfc" x="448" y="756">RFC 119</text>
      <rect class="mask" x="680" y="652" width="160" height="124" rx="6"/>
      <rect class="box loaf" x="680" y="652" width="160" height="124" rx="6"/>
      <text class="nm-a" x="696" y="682">The Loaf</text>
      <text class="sub" x="696" y="702">one sealed *.loaf unit</text>
      <text class="sub" x="696" y="718">payload · plan</text>
      <text class="sub" x="696" y="734">identity · receipt</text>
      <text class="rfc-a" x="696" y="756">RFC 117 · 124 · 123</text>
      <rect class="mask" x="888" y="652" width="328" height="124" rx="6"/>
      <rect class="box draft pr" x="888" y="652" width="328" height="124" rx="6"/>
      <text class="nm-a" x="904" y="680">Store</text>
      <text class="sub" x="904" y="698">Loaves keyed by unit identity</text>
      <text class="sub" x="904" y="712">external identity decides who rebakes</text>
      <text class="sub" x="904" y="726">shared across projects; collected by reach</text>
      <text class="sub" x="904" y="740">two-instance refusal at plan time</text>
      <text class="rfc-a" x="904" y="758">RFC 124 · CRASH-SAFE RFC 112</text>
      <rect class="mask" x="680" y="800" width="160" height="72" rx="6"/>
      <rect class="box" x="680" y="800" width="160" height="72" rx="6"/>
      <text class="nm"  x="696" y="826">Outputs</text>
      <text class="sub" x="696" y="844">binaries · carriers</text>
      <text class="rfc" x="696" y="864">LINKED, NOT COPIED</text>
      <!-- ---- Zone 4: Registry and sources ---- -->
      <rect class="mask" x="64" y="952" width="440" height="124" rx="6"/>
      <rect class="box draft" x="64" y="952" width="440" height="124" rx="6"/>
      <text class="nm"  x="80" y="982">Artifact graph</text>
      <text class="sub" x="80" y="1002">cards, relationships, discovery, advisories</text>
      <text class="sub" x="80" y="1018">AI assets: models, prompts, evals, agents</text>
      <text class="sub" x="80" y="1034">a projection of the event log</text>
      <text class="rfc" x="80" y="1056">RFC 079 · 080</text>
      <rect class="mask" x="784" y="952" width="432" height="124" rx="6"/>
      <rect class="box draft pr" x="784" y="952" width="432" height="124" rx="6"/>
      <text class="nmm-a" x="800" y="982">incan.pub</text>
      <text class="sub" x="800" y="1002">signed source Loaves + attested baked assets; assets are optional</text>
      <text class="sub" x="800" y="1018">sparse index; scoped names; append-only signed events</text>
      <text class="sub" x="800" y="1034">registry identity is a key, hostnames are transport; trusted publishing</text>
      <text class="rfc-a" x="800" y="1056">RFC 125 · SUPERSEDES RFC 034</text>
      <rect class="mask" x="64" y="1116" width="440" height="72" rx="6"/>
      <rect class="box draft" x="64" y="1116" width="440" height="72" rx="6"/>
      <text class="nm"  x="80" y="1142">Mirrors and private registries</text>
      <text class="sub" x="80" y="1160">same protocol, own key</text>
      <text class="rfc" x="80" y="1180">STATIC COPY · SAME SIGNATURES</text>
      <rect class="mask" x="784" y="1116" width="432" height="72" rx="6"/>
      <rect class="box ext" x="784" y="1116" width="432" height="72" rx="6"/>
      <text class="nmm-x" x="800" y="1142">crates.io</text>
      <text class="sub-x" x="800" y="1160">consumed, never published to or mirrored</text>
      <text class="rfc-x" x="800" y="1180">EXTERNAL · SPARSE INDEX, SOURCE ARCHIVES</text>
      <!-- ---- Lane: Command surfaces ---- -->
      <rect class="mask" x="64" y="1284" width="544" height="84" rx="6"/>
      <rect class="box planned" x="64" y="1284" width="544" height="84" rx="6"/>
      <text class="nmm" x="80" y="1312">incan</text>
      <text class="sub" x="80" y="1330">check · fmt · lsp · inspect · explain — language and semantic tooling</text>
      <text class="sub" x="80" y="1346">uses the compiler service directly; delegates lifecycle to the Oven API</text>
      <text class="rfc" x="80" y="1362">NEVER A SECOND RESOLVER · NO incan bake</text>
      <rect class="mask" x="680" y="1284" width="536" height="84" rx="6"/>
      <rect class="box planned" x="680" y="1284" width="536" height="84" rx="6"/>
      <text class="nmm" x="696" y="1312">oven</text>
      <text class="sub" x="696" y="1330">init · add · lock · plan · bake · build · test · publish · yank · store</text>
      <text class="sub" x="696" y="1346">drives resolver, unit graph, executor, store, registry client</text>
      <text class="rfc" x="696" y="1362">NEVER INVOKES THE incan CLI</text>
      <!-- ================= LEGEND ================= -->
      <line x1="40" y1="1420" x2="1240" y2="1420" stroke="rgba(228,235,242,0.22)" stroke-width="0.8"/>
      <text class="zn" x="40" y="1448" letter-spacing="0.18em">LEGEND</text>
      <rect x="112" y="1436" width="24" height="12" rx="2" fill="none" stroke="rgba(228,235,242,0.50)" stroke-width="1"/>
      <text class="lg" x="144" y="1448">implemented</text>
      <rect x="248" y="1436" width="24" height="12" rx="2" fill="none" stroke="rgba(228,235,242,0.50)" stroke-width="1" stroke-dasharray="4,3"/>
      <text class="lg" x="280" y="1448">planned</text>
      <rect x="360" y="1436" width="24" height="12" rx="2" fill="none" stroke="rgba(228,235,242,0.50)" stroke-width="1" stroke-dasharray="1,3"/>
      <text class="lg" x="392" y="1448">draft</text>
      <rect x="452" y="1436" width="24" height="12" rx="2" fill="rgba(255,193,90,0.14)" stroke="#ffc15a" stroke-width="1" stroke-dasharray="1,3"/>
      <text class="lg" x="484" y="1448">in PR #1477 — RFC 124, RFC 125</text>
      <rect x="720" y="1436" width="24" height="12" rx="2" fill="rgba(228,235,242,0.05)" stroke="rgba(228,235,242,0.24)" stroke-width="1"/>
      <text class="lg" x="752" y="1448">external or legacy</text>
      <line x1="900" y1="1442" x2="932" y2="1442" stroke="#48f0ef" stroke-width="1" stroke-dasharray="4,3"/>
      <text class="lg" x="940" y="1448">consumed as source only</text>
      <line x1="1120" y1="1442" x2="1152" y2="1442" stroke="#c1c8d0" stroke-width="1" stroke-dasharray="4,3"/>
      <text class="lg" x="1160" y="1448">optional / passive</text>
      <rect x="112" y="1464" width="24" height="12" rx="2" fill="rgba(255,193,90,0.14)" stroke="#ffc15a" stroke-width="1"/>
      <text class="lg" x="144" y="1476">the Loaf — the unit Oven bakes, stores, reuses, and exchanges</text>
    </svg>
</div>
<figcaption><strong>The map.</strong> Facets select units and the manifest is resolved into a lock; the unit graph gives every unit an identity before anything compiles. The compiler service checks the Incan facet and emits Rust; the executor compiles each unit and seals it as a Loaf; the store keeps Loaves by identity and hands them back to any later plan that matches. Publication ships the source Loaf with its attested baked assets, and <code>incan.pub</code> admits an asset back only on exact unit identity. Cargo appears only on an explicit legacy path; crates.io is consumed as source and never mirrored.</figcaption>
</figure>

## Inside a Loaf

A Loaf is not a cache guess. Its identity binds the semantic digest of its source, the dependency lock, the compiler and SDK, the target, the profile, and the resolved features, and its payload carries the compiled Rust library, the checked public surface, and an executable representation of its exports. A consumer either matches the sealed identity and reuses the result without Cargo, or receives a refusal that names the fact that differs and bakes only what is missing.

<figure class="inc-oven-map">
<div class="inc-oven-map__scroll">
<svg class="inc-oven-map__svg inc-oven-map__svg--anatomy" viewBox="0 0 1320 520" xmlns="http://www.w3.org/2000/svg" role="img" aria-labelledby="oven-loaf-title oven-loaf-desc">
  <title id="oven-loaf-title">Inside a Loaf</title>
  <desc id="oven-loaf-desc">A Loaf cut open into four layers: an identity over the effective compilation inputs, a payload holding the compiled Rust library, checked metadata and the executable representation of exports, the direct rustc plan that produced it, and the receipt that explains it; when published it also carries publisher and registry attestations. Beside it, a consumer's requested environment is compared with the sealed identity: a match reuses the Loaf without Cargo, a miss is a precise refusal that bakes only the missing units.</desc>
  <defs>
    <marker id="oven-loaf-arrow" markerWidth="8" markerHeight="6" refX="7" refY="3" orient="auto"><polygon points="0 0, 8 3, 0 6" fill="#c1c8d0"/></marker>
    <marker id="oven-loaf-arrow-a" markerWidth="8" markerHeight="6" refX="7" refY="3" orient="auto"><polygon points="0 0, 8 3, 0 6" fill="#ffc15a"/></marker>
  </defs>
  <rect width="100%" height="100%" fill="#030304"/>
  <rect class="box loaf" x="40" y="40" width="720" height="440" rx="8"/>
  <text class="nm-a" x="60" y="68">Inside a Loaf</text>
  <text class="rfc" x="180" y="68">LOCKED, OBSERVABLE ARTIFACT FORMAT · ONE FILE, FOUR THINGS</text>
  <rect class="layer" x="60" y="86" width="680" height="80" rx="5"/>
  <text class="nm" x="76" y="110">Identity</text>
  <text class="rfc" x="76" y="128">WHAT MADE IT</text>
  <text class="sub" x="250" y="106">a digest over the effective inputs: semantic digest of the source, dependency lock,</text>
  <text class="sub" x="250" y="122">compiler and SDK, target, profile, features, provider receipts</text>
  <text class="sub-x" x="250" y="150">same inputs on any machine, same identity — that is what makes reuse safe</text>
  <rect class="layer" x="60" y="176" width="680" height="94" rx="5"/>
  <text class="nm" x="76" y="200">Payload</text>
  <text class="rfc" x="76" y="218">WHAT IT IS</text>
  <text class="sub" x="250" y="196">compiled Rust library: rlib and rmeta</text>
  <text class="sub" x="250" y="212">checked .incnlib metadata: the public surface by canonical identity</text>
  <text class="sub" x="250" y="228">executable representation of exports, so non-Rust routes can run them (RFC 123)</text>
  <text class="sub-x" x="250" y="254">provider sidecars when the unit is a build script or a proc-macro host</text>
  <rect class="layer" x="60" y="280" width="680" height="60" rx="5"/>
  <text class="nm" x="76" y="304">Plan</text>
  <text class="rfc" x="76" y="322">HOW TO REPLAY IT</text>
  <text class="sub" x="250" y="304">the exact direct rustc invocation that produced the payload</text>
  <text class="sub-x" x="250" y="326">replayable without Cargo; inspectable with oven store inspect</text>
  <rect class="layer" x="60" y="350" width="680" height="60" rx="5"/>
  <text class="nm" x="76" y="374">Receipt</text>
  <text class="rfc" x="76" y="392">WHY IT EXISTS</text>
  <text class="sub" x="250" y="374">inputs, decisions, backend selection, and what may reuse the result</text>
  <text class="sub-x" x="250" y="396">one receipt answers “why does this artifact exist?”</text>
  <rect class="layer attest" x="60" y="420" width="680" height="44" rx="5"/>
  <text class="nm" x="76" y="447">Attestation</text>
  <text class="sub" x="250" y="441">added when published: the publisher's signature and the registry's (RFC 125)</text>
  <text class="sub-x" x="250" y="458">verified on import; the registry is a key, hostnames are transport</text>
  <text class="zn" x="820" y="66">WHEN A CONSUMER BUILDS, RUNS, OR TESTS</text>
  <rect class="mask" x="820" y="86" width="460" height="60" rx="6"/>
  <rect class="box" x="820" y="86" width="460" height="60" rx="6"/>
  <text class="nm" x="836" y="110">The requested environment</text>
  <text class="sub" x="836" y="130">its own source, lock, compiler, SDK, target, profile, features</text>
  <line class="e" x1="1050" y1="146" x2="1050" y2="190" marker-end="url(#oven-loaf-arrow)"/>
  <path class="e a" d="M 740,126 H 782 Q 790,126 790,134 V 210 Q 790,218 798,218 H 820" marker-end="url(#oven-loaf-arrow-a)"/>
  <rect class="mask" x="748" y="160" width="56" height="12" rx="2"/>
  <text class="al" x="776" y="169" text-anchor="middle">IDENTITY</text>
  <rect class="mask" x="820" y="190" width="460" height="56" rx="6"/>
  <rect class="box" x="820" y="190" width="460" height="56" rx="6"/>
  <text class="nm" x="836" y="214">Compare with the sealed identity</text>
  <text class="sub" x="836" y="234">no guessing that yesterday's artifact is probably fine</text>
  <line class="e" x1="940" y1="246" x2="940" y2="300" marker-end="url(#oven-loaf-arrow)"/>
  <rect class="mask" x="948" y="266" width="40" height="12" rx="2"/>
  <text class="al" x="952" y="275">MATCH</text>
  <line class="e" x1="1160" y1="246" x2="1160" y2="300" marker-end="url(#oven-loaf-arrow)"/>
  <rect class="mask" x="1168" y="266" width="32" height="12" rx="2"/>
  <text class="al" x="1172" y="275">MISS</text>
  <rect class="mask" x="820" y="300" width="216" height="164" rx="6"/>
  <rect class="box loaf" x="820" y="300" width="216" height="164" rx="6"/>
  <text class="nm-a" x="836" y="326">Reuse</text>
  <text class="sub" x="836" y="348">link the sealed result</text>
  <text class="sub" x="836" y="364">no Cargo, no recompilation</text>
  <text class="sub" x="836" y="380">from this machine's store or</text>
  <text class="sub" x="836" y="396">imported from incan.pub</text>
  <text class="sub-x" x="836" y="424">the receipt says why reuse</text>
  <text class="sub-x" x="836" y="440">was allowed</text>
  <rect class="mask" x="1064" y="300" width="216" height="164" rx="6"/>
  <rect class="box" x="1064" y="300" width="216" height="164" rx="6"/>
  <text class="nm" x="1080" y="326">Precise refusal</text>
  <text class="sub" x="1080" y="348">names the fact that differs:</text>
  <text class="sub" x="1080" y="364">toolchain, target, profile,</text>
  <text class="sub" x="1080" y="380">features, lock, or source</text>
  <text class="sub" x="1080" y="396">then bakes only the missing units</text>
  <text class="sub-x" x="1080" y="424">everything that still matches</text>
  <text class="sub-x" x="1080" y="440">is reused</text>
</svg>
</div>
<figcaption><strong>Four things in one file.</strong> Identity says what made it, the payload is what it is, the plan says how to replay it, and the receipt says why it exists. RFC 117 defines the asset, RFC 124 the identity, RFC 123 the executable representation, and RFC 125 the attestations a published Loaf gains.</figcaption>
</figure>

## Where each box is specified

| Component | Status | RFCs |
| --- | --- | --- |
| Incan facet | implemented | RFC 077, RFC 113 |
| Rust facet | planned | RFC 119; legacy crate dependencies RFC 013 |
| `loaf.toml` and `oven.lock` | planned | RFC 117; lock semantics RFC 020 |
| Project lifecycle | draft, v0.6 slice 5 for RFC 073, 076, 078 | RFC 073, 074, 075, 076, 078 |
| Unit graph and host providers | planned | RFC 119 |
| Resolver | planned | RFC 117; offline and locked builds RFC 020 |
| SDK components and compiled providers | implemented | RFC 114 |
| Cargo compatibility mode | planned, explicit adoption only | RFC 119 |
| Compiler service | implemented, extended by planned RFCs | RFC 120, 106, 123, 097, 116, 121 |
| Direct `rustc` executor | Oven Alpha today; planned scope in RFC 119 | RFC 119 |
| The Loaf | Oven Alpha today; identity in review | RFC 117, RFC 124, RFC 123 |
| Store | Oven Alpha today; unit identity in review | RFC 124; crash-safe publication RFC 112 |
| Outputs | implemented | RFC 117 |
| Artifact graph | draft | RFC 079, RFC 080 |
| `incan.pub` | draft, in review | RFC 125, supersedes RFC 034 |
| Mirrors and private registries | draft, in review | RFC 125 |
| crates.io | external, consumed as source only | RFC 119, RFC 125 |
| Command surfaces `incan` and `oven` | planned | RFC 118 |

The positioning behind the registry tier is in [Ship the loaf, not the recipe](../../whitepapers/incan_pub_ship_the_loaf.md); the toolchain that produces the Loaves is in [A Cargo-free toolchain for Incan and Rust](../../whitepapers/incan_oven_positioning.md); what ships today is on the [Oven Alpha](oven_alpha.md) page.
