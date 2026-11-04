# Issue tracker: GitHub

Issues and specs for this repository live in GitHub Issues. Use the `gh` CLI for all operations; infer the repository from the `origin` remote.

## Conventions

- **Create an issue:** `gh issue create --title "..." --body "..."` (use a heredoc for multiline bodies).
- **Read an issue:** `gh issue view <number> --json number,title,body,labels,comments`.
- **List issues:** `gh issue list --state open --json number,title,body,labels,comments` with appropriate label/state filters.
- **Comment:** `gh issue comment <number> --body "..."`.
- **Labels:** `gh issue edit <number> --add-label "..."` / `--remove-label "..."`.
- **Close:** `gh issue close <number> --comment "..."`.

## Pull requests as a triage surface

**PRs as a request surface: no.** PRs are not treated as feature requests for triage.

## Wayfinding operations

A Wayfinder map is one GitHub issue labelled `wayfinder:map`; its body is the canonical index of the destination, notes, decisions, fog, and out-of-scope work. Decision tickets are child issues labelled with exactly one of `wayfinder:research`, `wayfinder:prototype`, `wayfinder:grilling`, or `wayfinder:task` (along with no non-Wayfinder labels).

- **Link a ticket to its map:** create a GitHub sub-issue with `gh issue create --parent <map-number> ...` where supported; otherwise add the ticket to the map's task list and put `Part of #<map-number>` at the top of its body. `gh issue edit <map> --add-sub-issue <child>` is another supported path on recent `gh` versions.
- **Blocking:** use GitHub's native issue dependencies so the tracker UI shows the decision frontier. Add a dependency with `gh api --method POST repos/{owner}/{repo}/issues/<child>/dependencies/blocked_by -F issue_id=<blocker-database-id>`. Obtain the blocker database id with `gh api repos/{owner}/{repo}/issues/<number> --jq .id`; do not use the issue number or node id. If native dependencies are unavailable, use a `Blocked by: #<number>` body convention.
- **Frontier:** among open child tickets, a ticket is available when it has no open blockers and no assignee. Choose the first available ticket in map order unless the user names one.
- **Claim:** assign the selected ticket to the driving developer before doing work (`gh issue edit <number> --add-assignee @me`).
- **Resolve:** post the answer as a comment, close the issue with a resolution comment, then append a concise linked context pointer to the map's Decisions-so-far. Keep the detailed decision in the ticket; the map only indexes it.

GitHub issue dependencies and sub-issues depend on repository/API support. If unavailable, use the body conventions above and document the limitation rather than creating unrelated links.
