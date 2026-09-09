#!/usr/bin/env bash
# Classify CI work from the event and changed paths; emit GitHub job outputs.
set -euo pipefail

docs_only=false
if [ "$EVENT_NAME" = "pull_request" ]; then
  git fetch origin "$BASE_REF" --depth=1
  comparison_ref="origin/$BASE_REF"
  changed=false
  docs_only=true

  while IFS= read -r -d '' file; do
    changed=true
    # The two generated reference pages are derived from the compiler,
    # so they must retain the full generated-reference check.
    case "$file" in
      workspaces/docs-site/docs/language/reference/language.md|workspaces/docs-site/docs/language/reference/feature_inventory.md|workspaces/docs-site/docs/_snippets/language/examples/verified_*.incn)
        docs_only=false
        break
        ;;
      workspaces/docs-site/*)
        ;;
      *)
        docs_only=false
        break
        ;;
    esac
  done < <(git diff --name-only -z "$comparison_ref" HEAD)

  if [ "$changed" = false ]; then
    docs_only=false
  fi
fi

echo "docs_only=$docs_only" >> "$GITHUB_OUTPUT"

# The expensive half of this workflow -- four Oven replay shards, the prewarm that feeds them, the platform
# ABI matrix, the release-Loaf guard and the shadow-comparison evidence -- costs more than everything else
# combined. Spending it on every push to a development line pays release-gate prices for work that is not a
# release candidate yet. Run it where it decides something: an integration branch, a slice someone declares
# ready, or an explicit request.
heavy=false
case "$EVENT_NAME" in
  workflow_dispatch)
    [ "$DISPATCH_HEAVY" = "true" ] && heavy=true
    ;;
  pull_request)
    # A pull request into `main` or a release branch is a release candidate. One into a development line
    # is not, until it is labelled `full-ci`.
    case "$BASE_REF" in
      main|release/*) heavy=true ;;
    esac
    case ",$PR_LABELS," in
      *,full-ci,*) heavy=true ;;
    esac
    ;;
  *)
    case "$REF_NAME" in
      main|release/*) heavy=true ;;
    esac
    ;;
esac

echo "heavy=$heavy" >> "$GITHUB_OUTPUT"

# Heavy coverage always requires the compiler/SDK producer. Dispatch can select its docs/reference
# consumers alone, so cold/warm SDK experiments do not repeat the complete Oven suite.
reference="$heavy"
if [ "$EVENT_NAME" = "workflow_dispatch" ] && [ "$DISPATCH_REFERENCE" = "true" ]; then
  reference=true
fi
if [ "$reference" != "true" ] && [ "$EVENT_NAME" = "pull_request" ]; then
  while IFS= read -r -d '' file; do
    case "$file" in
      crates/incan_core/*) reference=true; break ;;
      workspaces/docs-site/docs/language/reference/language.md) reference=true; break ;;
      workspaces/docs-site/docs/language/reference/feature_inventory.md) reference=true; break ;;
      workspaces/docs-site/docs/_snippets/language/examples/*) reference=true; break ;;
    esac
  done < <(git diff --name-only -z "$comparison_ref" HEAD)
fi

echo "reference=$reference" >> "$GITHUB_OUTPUT"
