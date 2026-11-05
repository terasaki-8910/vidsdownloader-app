# Personal UI direction  (place at ~/.claude/rules/ui.md, NOT in the project repo)
# To load this ONLY for frontend files (saves context), add the path glob per the
# Claude Code rules-directory docs, e.g. scope to **/*.{tsx,jsx,css,scss}.
# Keep this file SHORT and specific -- vague rules get ignored, and everything here
# loads into context. Machine-checkable rules belong in the LINTER, not here.

## Aesthetic direction  (subjective; this is the human-taste layer)
- Do NOT look "AI-generated": no gratuitous gradients, no emoji, no filler copy.
- Prefer restraint: clear hierarchy, generous spacing, one accent color.
- <add your accumulated preferences here over time; prune with /memory>

## Hard rules (also enforced by lint -- listed here for the model's benefit)
- Colors only via design tokens. Never hardcode hex.
- Every interactive element keyboard-reachable; meet WCAG AA contrast.
- Responsive: verify at mobile / tablet / desktop breakpoints.
- In a list/card of variable-height items (e.g. a row whose content can wrap to several
  lines), per-row action controls (close/delete/export/etc.) must stay at a fixed,
  predictable position -- never vertically centered against the row's full height, which
  drags them far from a click a user expects near the top. Anchor them to a fixed edge
  (e.g. `align-items: flex-start` plus their own fixed height/padding), independent of
  how tall the row's content grows. Test against a real long-content case, not just the
  short placeholder row a mockup usually shows.
