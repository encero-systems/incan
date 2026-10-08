# Oven page design QA

## Source and implementation

- Selected direction: `/Users/danny/.codex/generated_images/019f5058-28ac-75a3-9dc6-810ce2113782/exec-2d27edbb-bda1-4bd6-8e46-eb13d45a0268.png`
- Implemented page: `http://127.0.0.1:8000/tooling/explanation/oven_alpha/`
- Desktop hero capture: `/Users/danny/Development/encero/tmp/incan-docs-rescope-831/oven-reference-fit-hero.jpg`
- Desktop architecture capture: `/Users/danny/Development/encero/tmp/incan-docs-rescope-831/oven-reference-fit-architecture-final.jpg`
- Desktop closing capture: `/Users/danny/Development/encero/tmp/incan-docs-rescope-831/oven-reference-fit-closing.jpg`
- Focused reference comparison: `/Users/danny/Development/encero/tmp/incan-docs-rescope-831/oven-reference-fit-comparison.jpg`
- Mobile hero capture: `/Users/danny/Development/encero/tmp/incan-docs-rescope-831/oven-reference-fit-mobile-top.jpg`
- Mobile architecture capture: `/Users/danny/Development/encero/tmp/incan-docs-rescope-831/oven-reference-fit-mobile-architecture.jpg`
- Revised story capture: `/Users/danny/Development/encero/tmp/incan-docs-rescope-831/oven-story-v18-top.png`
- Revised architecture capture: `/Users/danny/Development/encero/tmp/incan-docs-rescope-831/oven-story-v18-architecture.png`
- Revised comparison capture: `/Users/danny/Development/encero/tmp/incan-docs-rescope-831/oven-story-v18-comparison.png`
- Revised technical bridge capture: `/Users/danny/Development/encero/tmp/incan-docs-rescope-831/oven-story-v18-technical.png`
- Revised mobile capture: `/Users/danny/Development/encero/tmp/incan-docs-rescope-831/oven-story-v18-mobile.png`
- Final manifesto capture: `/Users/danny/Development/encero/tmp/incan-docs-rescope-831/oven-story-v19-manifesto-final.png`
- Final compatibility-decision capture: `/Users/danny/Development/encero/tmp/incan-docs-rescope-831/oven-story-v19-decision-final.png`
- Final mobile hero capture: `/Users/danny/Development/encero/tmp/incan-docs-rescope-831/oven-story-v19-mobile-top.png`
- Final mobile decision capture: `/Users/danny/Development/encero/tmp/incan-docs-rescope-831/oven-story-v19-mobile-decision.png`

## Test states

- Desktop: 1264 x 1204 CSS pixels.
- Annotated architecture regression: 1048 x 1087 CSS pixels.
- Mobile: 390 x 844 CSS pixels.
- Wide fullscreen source evidence: 4020 x 2490 device pixels, provided by the user.
- Page state: default dark theme with the technical reference visible.
- Horizontal overflow: none at either tested viewport.
- Browser connector telemetry did not expose the JavaScript console, so this report does not claim a console-clean result.

## Fidelity review

The final pass follows the reference's composition rather than merely borrowing its colors:

- the existing Incus-and-oven hero remains the dominant visual anchor;
- the short product definition, compact route actions, and explicit Alpha boundary remain in the first scene;
- `One system, three ways in` uses three large route cards with command-level evidence;
- the manifesto acts as a deliberate pause between navigation and architecture;
- the five-stage architecture is compact and sequential;
- publisher inputs, the bread Loaf, consumer outcomes, and the live inspector form one continuous visual system;
- dedicated cyan and gold connector artwork runs behind the semantic HTML cards without becoming the content itself;
- the central Loaf is unmistakably bread and is labelled `Locked, Observable Artifact Format`;
- the Cargo comparison, current Alpha surface, and planned north star use the same three-column hierarchy as the reference;
- the closing command/boundary scene reuses the hero artwork without placing Incus over text;
- deeper technical material remains available in a collapsed disclosure below the product narrative.

## Iterations performed

1. Reweighted the hero so the artwork and headline share the scene instead of behaving like a docs banner.
2. Enlarged the three entry routes and reduced generic card styling.
3. Rebuilt the architecture around a physical bread Loaf and an explicit publisher-to-consumer path.
4. Corrected the architecture grid so connector artwork cannot consume layout columns or inflate the Loaf.
5. Added small, local source/outcome icons and kept their cyan/gold semantics consistent.
6. Added a compact live inspector and a separate evidence/miss ledger.
7. Matched the Cargo/current/planned three-card hierarchy while keeping planned capabilities explicitly unavailable in Alpha.
8. Reflowed the architecture to two columns and then one column, hiding connector decoration when it no longer clarifies the flow.
9. Verified route-card navigation, the technical-reference disclosure, reduced-motion handling, and no horizontal overflow.
10. Corrected the wide-screen hero crop by preserving additional hero height and top-anchoring the 2:1 source at fullscreen widths.
11. Replaced the collapsed technical reference with visible content and added a sticky Oven section navigator, so CLI and proof routes land on readable sections with navigation still present.
12. Replaced the opaque Loaf crop with a true RGBA cutout, preserving the stamped bread and its cyan/amber material lighting.
13. Split the architecture connector field into independently positioned publisher and consumer overlays, aligning three inputs and five outcomes without drawing through card copy or the inspector.
14. Made the architecture cards opaque enough to mask decorative routing underneath while retaining the dark glass-panel treatment.
15. Turned the quiet link strip into an explicitly labelled, segmented `Explore Oven` navigator and verified its architecture anchor against the sticky header.
16. Replaced the context-free five-step rail with a three-part causal story: publisher preparation, sealed proof, and consumer reuse or refusal.
17. Restored explanatory prose around proof contents, reuse misses, and the Cargo comparison while increasing the small-text floor at the annotated 1048 x 1087 viewport.
18. Added a visual `Use → Inspect → Decide` bridge into the technical reference and removed the weak standalone Incus portrait from the Alpha-boundary callout.
19. Changed the hero kicker from maturity-first language to the product promise, `A better build system`, while preserving the explicit Alpha boundary in the hero copy.
20. Replaced the neutral manifesto portrait with an inspecting Incus and sized/dimmed the transparent asset for the card rather than treating it as a floating icon.
21. Replaced the ambiguous `Cargo builds. Oven remembers.` line with `One build. A reusable result with receipts.` and clarified that Cargo is a bounded Alpha publisher rather than the consumer build path.
22. Removed the dangling bottom technical-reference island and redistributed its mechanics into visible command, evidence, and compatibility-decision sections within the product story.

## Verification

- `make docs-check-components` passed.
- `make docs-build` passed, including component and learning-journey contracts.
- `git diff --check` passed.
- The final preview loaded the `learning-refresh-17` stylesheet, the transparent Loaf asset, and both split connector assets.
- In-app browser inspection at the reported 1048 x 1087 viewport confirmed that the bread no longer has a rectangular background and that all connector endpoints align with their corresponding input/outcome rows.
- In-app browser inspection at 1048 x 1087 confirmed the revised navigation, three-part proof story, enlarged explanatory text, Cargo/current/planned hierarchy, and technical-story bridge with no horizontal overflow.
- The architecture anchor lands 61 CSS pixels below the sticky header, and the in-app browser reported no error-level console messages.
- Mobile inspection at 390 x 844 confirmed one-column story and architecture grids with no page-level horizontal overflow; the labelled local navigator intentionally scrolls horizontally rather than compressing its labels.
- Final desktop inspection at 1280 x 720 confirmed the new inspecting Incus, revised ownership headline, visible command/evidence/decision mechanics, and zero missing images or page-level overflow.
- Final mobile inspection at 390 x 844 confirmed that the decision cards stack, the decorative manifesto Incus hides, all copy remains readable, and the page has no horizontal overflow.

## Final visual check

The annotated story and consistency issues are resolved in the refreshed local preview. The page now explains one causal loop—prepare once, seal the proof, then reuse or refuse—while mixing commands, evidence, compatibility decisions, and boundary details into the main narrative. The Cargo comparison now makes the Oven ownership boundary explicit, and the new manifesto Incus supports the meaning of the quote without dominating it. Decorative connectors remain hidden at the narrow breakpoint, where the board reflows and the lines would no longer improve comprehension.

final result: passed

# Incan v0.5 release page design QA

## Story and implementation

- Implemented page: `http://127.0.0.1:8000/release_notes/0_5/`
- Campaign promise: `Readable source. Real native systems.`
- Primary first-run path: the typed data processor tutorial.
- Supporting routes: native API, checked package, and Rust-crate integration.
- Oven remains a supporting Alpha foundation rather than the release headline.
- The full issue-level inventory remains available in a collapsed disclosure for upgrade and debugging work.

## Test states

- Desktop: 1440 x 1000 CSS pixels.
- Mobile: 390 x 844 CSS pixels.
- Page state: default dark theme, navigation and table of contents intentionally hidden for the campaign layout.
- Horizontal overflow: none at either tested viewport.
- Missing images: none.

## Interaction and accessibility checks

- The `Test drive` local-navigation action lands below the sticky header at desktop and mobile widths.
- The flagship-project CTA navigates to `/language/tutorials/typed_data_processor/`.
- The local release navigator remains horizontally scrollable on mobile rather than compressing its labels.
- Outcome cards retain visible focus and hover affordances and keep the full card as the link target.
- Decorative Incus artwork reserves its own space and does not overlap proof results or closing copy.
- Custom release-page microcopy has a 12 CSS-pixel minimum at the desktop root size; ordinary prose renders at 15.84 CSS pixels.
- All section anchors account for the sticky header through scoped `scroll-margin-top` rules.
- Reduced-motion users do not receive outcome-card motion.

## Verification

- `make docs-build` passed, including the Incapunk component and learning-journey contracts.
- `git diff --check` passed.
- Desktop and mobile browser inspection confirmed responsive stacking, readable proof code, visible outcome icons, and no page-level overflow.
- Browser interaction confirmed the local anchor and flagship tutorial route.

final result: passed
