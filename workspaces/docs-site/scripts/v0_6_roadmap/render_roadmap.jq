def slices:
  [
    { number: 1, issue_number: 1137, slug: "cutover-foundation", title: "Cutover foundation", focus: "Cutover facts, Body IR, and parity.", proof: "Selected and executed routes are receipted; unavailable comparisons stay non-green.", exit_evidence: "backend selection and execution are explicit; parity records have stable IDs; unavailable or skipped comparisons remain non-green." },
    { number: 2, issue_number: 1138, slug: "first-executable-proof", title: "First executable proof", focus: "Direct execution of a bounded Body-IR program.", proof: "Source spans, semantic facts, and paired receipts survive execution.", exit_evidence: "a bounded Body-IR program executes through the replacement route with source spans, ownership/runtime facts, and paired receipts." },
    { number: 3, issue_number: 1140, slug: "language-runtime-matrix", title: "Language and runtime matrix", focus: "Embedded fragments, capabilities, and package vocabulary.", proof: "Accepted constructs have parity, diagnostics, inspection, and migration evidence.", exit_evidence: "every accepted construct has parity coverage, diagnostics, inspection output, formatter/LSP proof where relevant, and an explicit migration disposition." },
    { number: 4, issue_number: 1141, slug: "oven-authority-native-rust", title: "Oven authority and native Rust", focus: "Loaf authority, governed providers, and bounded Rust facets.", proof: "Incan-only, Rust-only, and mixed Loaves bake without Cargo being authoritative.", exit_evidence: "Rust-only, Incan-only, and mixed Loaves bake without Cargo being the project authority; Cargo remains valid as explicit compatibility/adoption mode." },
    { number: 5, issue_number: 1142, slug: "oven-cli-delivery", title: "Oven CLI delivery", focus: "Shared Oven planning for build and bake.", proof: "One plan and receipt model; neither CLI gains a competing planner.", exit_evidence: "one installation, two non-competing CLIs, one Loaf authority, and one plan/receipt model." },
    { number: 6, issue_number: 1143, slug: "first-class-inspectability", title: "First-class inspectability", focus: "Shared compiler, package, artifact, and Rust/Oven inspection facts.", proof: "CLI, LSP, Architect, MCP, and Rust inspection agree on identity and provenance.", exit_evidence: "CLI, LSP, Architect, MCP, and Rust inspection agree on identity, range, provenance, and stale-state semantics." },
    { number: 7, issue_number: 1139, slug: "canonical-source-meaning", title: "Canonical source meaning", focus: "One source identity across compiler, tools, and backend facts.", proof: "Aliases, imports, locals, members, and binders resolve consistently.", exit_evidence: "aliases, imports, re-exports, locals, members, and generic binders resolve to one identity across compiler, LSP, graph, and backend facts." },
    { number: 8, issue_number: 1379, slug: "native-windows", title: "Native Windows support", focus: "A Windows host that builds, bakes, and ships like the others.", proof: "A packaged Windows toolchain builds and runs a project, not merely a bake that exits zero.", exit_evidence: "the release bake produces a Windows archive; that archive, extracted and installed, scaffolds a project and builds it; platform-gated behaviour is implemented or its divergence is stated in code and docs." },
    { number: 9, issue_number: 1144, slug: "cutover-release", title: "Cutover and release", focus: "Corpus-led compatibility reporting and normal-path removal.", proof: "The complete matrix is green or explicitly migrated before generated Rust loses authority.", exit_evidence: "generated Rust is an inspection/debug projection only; normal compilation, package contracts, and Oven no longer depend on it as semantic authority." }
  ];

def parent_number:
  (.parent_issue_url? // "") | split("/") | last | tonumber?;

def children($issues; $parent):
  [$issues[] | select(parent_number == $parent)];

def descendants($issues; $parent):
  children($issues; $parent) as $children
  | $children + [$children[] | descendants($issues; .number)[]];

def tree_issues($issues; $root):
  ([$issues[] | select(.number == $root)] + descendants($issues; $root))
  | unique_by(.number);

def root_issue($issues; $number):
  first($issues[] | select(.number == $number))
  // error("the v0.6 milestone snapshot does not contain slice root #\($number)");

def node_id($root):
  if .number == $root then "s\(.number)" else "i\(.number)" end;

def number_node_id($number; $root):
  if $number == $root then "s\($number)" else "i\($number)" end;

def graph_title:
  sub("^(feature|bugfix|chore|docs|refactor) - "; "")
  | if length > 42 then .[0:39] + "…" else . end
  | gsub("\\\\"; "\\\\\\\\")
  | gsub("\\\""; "\\\\\"")
  | gsub("[\\r\\n]+"; " ");

def blocker_nodes($nodes; $blocked_by):
  [$nodes[] as $issue
   | ($blocked_by[($issue.number | tostring)] // [])[]
   | select(.number as $number | ($nodes | map(.number) | index($number) | not))]
  | unique_by(.number);

def missing_blocker_relationships($issues; $blocked_by):
  [$issues[]
   | select(.issue_dependencies_summary.blocked_by > 0)
   | select($blocked_by[(.number | tostring)] == null)];

def node_label($node; $root; $slice_number):
  if $node.number == $root
  then "Slice \($slice_number)<br/>#\($node.number)"
  else "#\($node.number)<br/>\($node.title | graph_title)"
  end;

def render_node($node; $root; $slice_number):
  "  \($node | node_id($root))[\"\(node_label($node; $root; $slice_number))\"]\n";

def render_graph($spec; $nodes; $blocked_by):
  $spec.issue_number as $root
  | blocker_nodes($nodes; $blocked_by) as $external_blockers
  | ($nodes + $external_blockers | unique_by(.number)) as $all_nodes
  | ($all_nodes | map(select(.state == "closed") | node_id($root))) as $complete_ids
  | "```mermaid\nflowchart LR\n"
  + ($all_nodes | map(render_node(.; $root; $spec.number)) | join(""))
  + ([$nodes[]
      | select(.number != $root)
      | parent_number as $parent
      | "  \(number_node_id($parent; $root)) -- owns --> \(node_id($root))\n"] | join(""))
  + ([$nodes[] as $issue
      | ($blocked_by[($issue.number | tostring)] // [])[]
      | "  \(node_id($root)) -. blocks .-> \($issue | node_id($root))\n"] | join(""))
  + (if ($complete_ids | length) > 0
     then "  classDef incv06complete fill:#0b2724,stroke:#66d9a3,color:#e4ebf2,stroke-width:1.7px\n  class \($complete_ids | join(",")) incv06complete\n"
     else ""
     end)
  + ($all_nodes | map("  click \(node_id($root)) href \"\(.html_url)\" \"Open #\(.number) on GitHub\"\n") | join(""))
  + "```\n";

def roadmap_table_row($spec; $issues):
  tree_issues($issues; $spec.issue_number) as $nodes
  | ($nodes | map(select(.state == "closed")) | length) as $complete
  | ($nodes | map(select(.state == "open")) | length) as $open
  | "    <tr data-v06-slice-target=\"slice-\($spec.number | tostring | if length == 1 then "0" + . else . end)-\($spec.slug)\">\n"
  + "      <td><button type=\"button\" class=\"inc-v06-slice-toggle\">\($spec.number). \($spec.title)</button><div class=\"inc-v06-slice-status\"><span class=\"inc-v06-status inc-v06-status--complete\">\($complete) complete</span><span class=\"inc-v06-status inc-v06-status--open\">\($open) open</span></div></td>\n"
  + "      <td>\($spec.focus)</td>\n"
  + "      <td>\($spec.proof)</td>\n"
  + "    </tr>\n";

def roadmap_table($specs; $issues):
  "<div class=\"inc-v06-roadmap-table-wrap\">\n<table class=\"inc-v06-roadmap-table\">\n  <thead>\n    <tr>\n      <th scope=\"col\">Slice</th>\n      <th scope=\"col\">Focus</th>\n      <th scope=\"col\">Proof before moving on</th>\n    </tr>\n  </thead>\n  <tbody>\n"
  + ($specs | map(roadmap_table_row(.; $issues)) | join(""))
  + "  </tbody>\n</table>\n</div>\n";

def slice_detail($spec; $issues; $blocked_by):
  tree_issues($issues; $spec.issue_number) as $nodes
  | root_issue($issues; $spec.issue_number) as $root
  | "<details id=\"slice-\($spec.number | tostring | if length == 1 then "0" + . else . end)-\($spec.slug)\" class=\"inc-v06-slice\" markdown=\"1\"\(if $spec.number == 1 then " open" else "" end)>\n"
  + "<summary>\($spec.number). \($spec.title)</summary>\n\n"
  + "**Exit evidence:** \($spec.exit_evidence)\n\n"
  + render_graph($spec; $nodes; $blocked_by)
  + "[Open Slice \($spec.number) in GitHub](\($root.html_url))\n\n</details>\n";

($issues[0] | map(select(.pull_request == null))) as $all_issues
| ($blocked_by[0] // {}) as $blockers
| slices as $specs
| missing_blocker_relationships($all_issues; $blockers) as $missing_blockers
| "<!-- Generated by `scripts/v0_6_roadmap/render_roadmap.sh`; do not edit by hand. -->\n\n"
+ "**GitHub snapshot:** \($snapshot_date). In this map, **complete** means the GitHub issue is closed; **open** means it remains unfinished. Complete issue nodes are green; open nodes retain the gold outline.\n\n"
+ (if ($missing_blockers | length) > 0
   then "> **Blocked-by relationships are unavailable in this snapshot.** GitHub reports prerequisites for \($missing_blockers | length) issues, but their relationship records were not fetched; dashed edges are therefore omitted. Refresh with `GITHUB_TOKEN` before publishing.\n\n"
   else ""
   end)
+ roadmap_table($specs; $all_issues)
+ "\n"
+ ($specs | map(slice_detail(.; $all_issues; $blockers)) | join("\n"))
| rtrimstr("\n")
