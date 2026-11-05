# vidsdownloader — Claude Code

@AGENTS.md

## Decision sheets
The procedure (Japanese) is imported below. This project's one sheet is
`docs/decision-sheet.html`, republished every round to the same artifact:
https://claude.ai/artifact/1Y3XCBAGCybYvx9yLU3FQx (answers in its db under
`answers/round<N>`).
@docs/decision-sheet.md

## UI direction
@~/.claude/rules/ui.md

## Parallel work
Independent features that share no files can be fanned out with the Workflow tool
(subagents in per-feature worktrees) when the user asks for it; merge each back with
`git merge --no-ff` once its checks pass.

## Plan Mode
For a genuinely hard architecture/design call, switch to Fable manually before entering
Plan Mode (`/model fable`), then switch back afterward. Claude Code has no hook or
setting that binds a model to Plan Mode, so this is manual every time -- see
`~/.claude/CLAUDE.md`.

## Model / effort indicator
The status line shows the current model (and reasoning effort where exposed) -- see
`.claude/statusline.sh`. It is not repeated at the end of chat replies.

## Git
`git push` is allow-listed in `.claude/settings.json` only to remove friction once the
user asks for a push; it does not change the never-push-unasked rule.
